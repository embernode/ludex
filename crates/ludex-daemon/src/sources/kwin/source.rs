//! KWin foreground-window source.
//!
//! On daemon startup this source installs an embedded KWin JavaScript
//! that subscribes to `workspace.windowActivated` and calls back to the
//! daemon's D-Bus service (`net.ludex.Tracker1`,
//! `/net/ludex/ForegroundEvents`, `net.ludex.ForegroundEvents1`). Each
//! activation produces a gate decision; the resulting transition is
//! translated into `GameEvent::Started` / `GameEvent::Stopped` on the
//! shared event channel.
//!
//! A grace window between "tracked game loses foreground" and "emit
//! Stop" absorbs short alt-tabs — see [`transition`](super::transition)
//! for the rationale.
//!
//! Requires KDE Plasma 6+. The script API is compatible across 6.x
//! minor releases.

use std::future::pending;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{Context, Result};
use ludex_core::default_database_path;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch};
use tokio::time::Sleep;
use tracing::{debug, info, instrument, warn};
use zbus::Connection;

use crate::config::SharedConfig;
use crate::event::GameEvent;
use crate::gate::{Gate, GateInput};
use crate::proc::pidfd;

use super::transition::{
    next_action, on_grace_timeout, on_tracked_exit, FgState, ForegroundMeta, Outcome, TimerOp,
    TransitionEvent,
};

const PLUGIN_NAME: &str = "ludex-foreground";
// The KWin scripting sandbox in Plasma 6 silently drops callDBus to
// destinations outside the `org.kde.*` / `org.freedesktop.*` prefix
// space. The *public* ludex API (GUI ↔ daemon) still lives under
// `net.ludex.*` in M6; this service exists solely for the KWin script
// to call back into the daemon and is allowed to use the KDE prefix
// because that is its exclusive purpose.
const DBUS_SERVICE_NAME: &str = "org.kde.ludex.Tracker1";
const DBUS_OBJECT_PATH: &str = "/org/kde/ludex/ForegroundEvents";

const SCRIPT_JS: &str = include_str!("script.js");

/// Activation event forwarded from the KWin script to the daemon.
#[derive(Debug, Clone)]
pub(crate) struct Activation {
    /// Process id of the active window.
    pub pid: u32,
    /// Whether the window is fullscreen on its output.
    pub is_fullscreen: bool,
    /// `Window.resourceClass` from KWin.
    pub resource_class: String,
    /// `Window.caption` from KWin.
    pub caption: String,
}

/// zbus interface the KWin script calls.
struct ForegroundEvents {
    tx: mpsc::UnboundedSender<Activation>,
}

#[zbus::interface(name = "org.kde.ludex.ForegroundEvents1")]
impl ForegroundEvents {
    // All arguments are strings. KWin's `callDBus` is conservative
    // about automatic JS → D-Bus type coercion (a JS Number does not
    // reliably marshal as `u` or `b` in Plasma 6); strings always do.
    // The script stringifies pid and fullscreen; we parse them here.
    #[allow(clippy::unused_async, reason = "zbus interface methods are async")]
    async fn report_window_activated(
        &self,
        pid: String,
        is_fullscreen: String,
        resource_class: String,
        caption: String,
    ) {
        let Ok(pid) = pid.parse::<u32>() else {
            return;
        };
        let is_fullscreen = matches!(is_fullscreen.as_str(), "true" | "1");
        let _ = self.tx.send(Activation {
            pid,
            is_fullscreen,
            resource_class,
            caption,
        });
    }
}

/// Proxy for the subset of `org.kde.kwin.Scripting` this source uses.
///
/// KWin's methods are camelCase (`loadScript`, not `LoadScript`), so
/// each is given an explicit `name` — zbus's default PascalCase
/// translation would produce names the service doesn't export.
#[zbus::proxy(
    interface = "org.kde.kwin.Scripting",
    default_service = "org.kde.KWin",
    default_path = "/Scripting"
)]
trait KWinScripting {
    #[zbus(name = "loadScript")]
    fn load_script(&self, file_path: &str, plugin_name: &str) -> zbus::Result<i32>;
    #[zbus(name = "isScriptLoaded")]
    fn is_script_loaded(&self, plugin_name: &str) -> zbus::Result<bool>;
    #[zbus(name = "unloadScript")]
    fn unload_script(&self, plugin_name: &str) -> zbus::Result<bool>;
    #[zbus(name = "start")]
    fn start(&self) -> zbus::Result<()>;
}

/// The foreground-window source itself.
pub struct KWinForegroundSource {
    gate: Gate,
    /// Shared handle to the tunable daemon config. The source reads
    /// `alt_tab_grace` from it at the moment it arms a grace timer,
    /// so a setter RPC takes effect without a daemon restart.
    config: SharedConfig,
}

impl KWinForegroundSource {
    /// Construct a source with the given gate and shared config.
    #[must_use]
    pub const fn new(gate: Gate, config: SharedConfig) -> Self {
        Self { gate, config }
    }

    /// Return `true` if `org.kde.KWin` is present on the session bus,
    /// which is the precondition for installing the script. Lets the
    /// daemon suppress this source on non-Plasma desktops without
    /// logging an error at every start.
    pub async fn is_kwin_available() -> bool {
        let Ok(conn) = Connection::session().await else {
            return false;
        };
        let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else {
            return false;
        };
        let names = proxy.list_names().await.unwrap_or_default();
        names.iter().any(|n| n.as_str() == "org.kde.KWin")
    }

    /// Install the KWin script, register the D-Bus service, and drive
    /// the activation → transition → event pipeline until `shutdown`
    /// fires. On exit, emits a final `Stopped` for any still-tracked
    /// foreground and unloads the script.
    #[instrument(name = "kwin_source", skip_all)]
    pub async fn install_and_run(
        self,
        event_tx: mpsc::Sender<GameEvent>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        let self_pid = std::process::id();

        // Channel between the D-Bus handler and this task's event loop.
        let (activation_tx, mut activation_rx) = mpsc::unbounded_channel::<Activation>();

        // Register the D-Bus service, then (and only then) install the
        // KWin script. The script starts emitting immediately — the
        // service has to be up to receive the first activation.
        let dbus = zbus::connection::Builder::session()
            .context("connect to session bus")?
            .name(DBUS_SERVICE_NAME)
            .context("request bus name")?
            .serve_at(
                DBUS_OBJECT_PATH,
                ForegroundEvents {
                    tx: activation_tx.clone(),
                },
            )
            .context("register foreground-events interface")?
            .build()
            .await
            .context("start D-Bus service")?;

        install_script(&dbus).await.context("install KWin script")?;
        info!(plugin = PLUGIN_NAME, "KWin foreground script installed");

        let final_state = self
            .run_loop(&event_tx, &mut activation_rx, &mut shutdown, self_pid)
            .await;

        // On graceful shutdown, stop any currently-tracked foreground
        // (including one sitting in TrackedBackgrounded — the grace
        // window is moot when the daemon is going away).
        if let Some(af) = final_state.current() {
            let _ = event_tx
                .send(GameEvent::Stopped {
                    key: af.key.clone(),
                    at: OffsetDateTime::now_utc(),
                })
                .await;
        }

        // Best-effort script cleanup. If the user's KDE session has
        // already gone away, the unload errors are harmless.
        if let Err(e) = uninstall_script(&dbus).await {
            warn!(error = %e, "KWin script unload failed (harmless if session ended)");
        }
        drop(dbus);
        Ok(())
    }

    /// Run the activation → transition → event pipeline until
    /// `shutdown` fires or the activation channel closes. Returns
    /// the final tracked state so the caller can emit a closing
    /// `Stopped` on graceful exit.
    async fn run_loop(
        &self,
        event_tx: &mpsc::Sender<GameEvent>,
        activation_rx: &mut mpsc::UnboundedReceiver<Activation>,
        shutdown: &mut watch::Receiver<bool>,
        self_pid: u32,
    ) -> FgState {
        let mut state = FgState::NotTracked;
        let mut grace_timer: Option<Pin<Box<Sleep>>> = None;
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<u32>();

        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { break; }
                }
                Some(exited_pid) = exit_rx.recv() => {
                    let outcome = on_tracked_exit(
                        std::mem::replace(&mut state, FgState::NotTracked),
                        exited_pid,
                    );
                    apply_outcome(
                        outcome,
                        &mut state,
                        &mut grace_timer,
                        &self.config,
                        event_tx,
                        &exit_tx,
                    )
                    .await;
                }
                () = poll_grace_timer(&mut grace_timer) => {
                    // Timer just fired; drop it so the next select
                    // doesn't poll a completed future.
                    grace_timer = None;
                    let pause_when_backgrounded =
                        self.config.read().await.pause_when_backgrounded;
                    let outcome = on_grace_timeout(
                        std::mem::replace(&mut state, FgState::NotTracked),
                        pause_when_backgrounded,
                    );
                    apply_outcome(
                        outcome,
                        &mut state,
                        &mut grace_timer,
                        &self.config,
                        event_tx,
                        &exit_tx,
                    )
                    .await;
                }
                maybe = activation_rx.recv() => {
                    let Some(activation) = maybe else { break; };
                    self.handle_activation(
                        activation,
                        self_pid,
                        &mut state,
                        &mut grace_timer,
                        event_tx,
                        &exit_tx,
                    )
                    .await;
                }
            }
        }

        state
    }

    /// Drive a single KWin activation through the gate and the
    /// transition state machine.
    async fn handle_activation(
        &self,
        activation: Activation,
        self_pid: u32,
        state: &mut FgState,
        grace_timer: &mut Option<Pin<Box<Sleep>>>,
        event_tx: &mpsc::Sender<GameEvent>,
        exit_tx: &mpsc::UnboundedSender<u32>,
    ) {
        debug!(
            pid = activation.pid,
            fullscreen = activation.is_fullscreen,
            resource_class = %activation.resource_class,
            caption = %activation.caption,
            "foreground window activated"
        );
        if activation.pid == self_pid {
            return;
        }
        let decision = self
            .gate
            .decide(GateInput {
                pid: activation.pid,
                window_is_fullscreen: activation.is_fullscreen,
            })
            .await;
        debug!(pid = activation.pid, decision = ?decision, "gate decision");
        let meta = ForegroundMeta {
            pid: activation.pid,
            resource_class: activation.resource_class,
            caption: activation.caption,
        };
        let pause_when_backgrounded = self.config.read().await.pause_when_backgrounded;
        let outcome = next_action(
            std::mem::replace(state, FgState::NotTracked),
            &meta,
            decision,
            pause_when_backgrounded,
        );
        apply_outcome(outcome, state, grace_timer, &self.config, event_tx, exit_tx).await;
    }
}

/// Write `script.js` to a stable on-disk path and tell KWin to load it.
async fn install_script(conn: &Connection) -> Result<()> {
    let script_path = script_path()?;
    if let Some(parent) = script_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    tokio::fs::write(&script_path, SCRIPT_JS)
        .await
        .with_context(|| format!("write {}", script_path.display()))?;

    let proxy = KWinScriptingProxy::new(conn)
        .await
        .context("construct org.kde.kwin.Scripting proxy")?;

    // A prior ludex-daemon run may have left the script registered.
    // unloadScript is idempotent (returns false when not loaded).
    let _ = proxy.unload_script(PLUGIN_NAME).await;

    let path_str = script_path
        .to_str()
        .context("KWin script path is not valid UTF-8")?;
    let _id = proxy
        .load_script(path_str, PLUGIN_NAME)
        .await
        .context("loadScript")?;
    proxy.start().await.context("Scripting.start")?;
    Ok(())
}

/// Uninstall the script by name. Idempotent.
async fn uninstall_script(conn: &Connection) -> Result<()> {
    let proxy = KWinScriptingProxy::new(conn).await?;
    let _ = proxy.unload_script(PLUGIN_NAME).await;
    Ok(())
}

/// On-disk path the script is written to.
fn script_path() -> Result<PathBuf> {
    // We drop the script next to the database so the ludex data
    // directory stays self-contained. Using $XDG_DATA_HOME/ludex/ keeps
    // it isolated from KWin's own script directory, which expects a
    // plugin bundle format we don't match.
    let db = default_database_path().context("resolve database path")?;
    let dir = db
        .parent()
        .context("database path has no parent")?
        .to_path_buf();
    Ok(dir.join("kwin-foreground.js"))
}

/// Wait for the grace timer to fire, if one is armed. When none is
/// armed the future is pending forever, which is what we want in the
/// `tokio::select!` loop — the branch is inert until a timer exists.
async fn poll_grace_timer(timer: &mut Option<Pin<Box<Sleep>>>) {
    match timer {
        Some(sleep) => sleep.as_mut().await,
        None => pending::<()>().await,
    }
}

/// Process the events in `outcome`, roll forward `state`, and apply
/// the timer op. A newly-emitted `Start` arms a pidfd watcher on the
/// new pid so the session closes promptly if the process exits
/// without the foreground ever changing.
///
/// The grace duration is read from `config` at the moment the timer
/// is actually armed, so a `SetAltTabGraceSeconds` RPC lands on the
/// very next backgrounded tracked window.
async fn apply_outcome(
    outcome: Outcome,
    state: &mut FgState,
    grace_timer: &mut Option<Pin<Box<Sleep>>>,
    config: &SharedConfig,
    events: &mpsc::Sender<GameEvent>,
    exit_tx: &mpsc::UnboundedSender<u32>,
) {
    let now = OffsetDateTime::now_utc();
    let Outcome {
        events: transition_events,
        state: new_state,
        timer,
    } = outcome;

    for ev in transition_events {
        match ev {
            TransitionEvent::Start {
                key, display_name, ..
            } => {
                // The post-transition state is the one carrying the
                // just-started pid; grab it while we still have a
                // borrow on `new_state`.
                let pid = new_state.current().map(|af| af.pid);
                let _ = events
                    .send(GameEvent::Started {
                        key,
                        display_name,
                        at: now,
                    })
                    .await;
                if let Some(pid) = pid {
                    let _ = pidfd::watch(pid, exit_tx.clone());
                }
            }
            TransitionEvent::Stop { key } => {
                let _ = events.send(GameEvent::Stopped { key, at: now }).await;
            }
        }
    }

    *state = new_state;

    match timer {
        TimerOp::NoChange => {}
        TimerOp::Start => {
            let grace = config.read().await.alt_tab_grace;
            *grace_timer = Some(Box::pin(tokio::time::sleep(grace)));
        }
        TimerOp::Cancel => {
            *grace_timer = None;
        }
    }
}
