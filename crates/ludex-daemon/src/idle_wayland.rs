//! Wayland-native user-idle source via `ext-idle-notify-v1`.
//!
//! The primary idle signal on Wayland desktops. The logind `IdleHint`
//! watcher in [`crate::idle`] remains as the fallback, but on KDE
//! Plasma Wayland it never fires: KWin neither calls `SetIdleHint`
//! nor answers `GetSessionIdleTime`, so the property this daemon
//! watched stayed `false` for entire sessions and
//! `interactive_runtime_seconds` always equalled
//! `full_runtime_seconds`.
//!
//! This source binds `ext_idle_notifier_v1` at interface version 2
//! and uses `get_input_idle_notification` — the version-2 request
//! that tracks user input alone and **ignores idle inhibitors**.
//! That distinction is load-bearing: games and Steam set idle
//! inhibitors routinely, so the version-1 request (which honours
//! them) would reproduce the always-zero idle this source exists to
//! fix.
//!
//! Threading: all Wayland objects live on one dedicated worker
//! thread parked in `blocking_dispatch`. Events feed the shared
//! [`IdleTracker`] directly — the session manager samples that state
//! on its own schedule, so no channel to the async side is needed.
//!
//! Connection strategy: `WAYLAND_DISPLAY` is tried first, but a
//! systemd `--user` service execs before the session imports that
//! variable into the user manager's environment, and a process env
//! cannot be retro-filled — so the packaged daemon never has it. The
//! fallback connects `$XDG_RUNTIME_DIR/wayland-0` directly:
//! `XDG_RUNTIME_DIR` is set by systemd itself, and `wayland-0` is the
//! default socket name Plasma's compositor listens on. Transient
//! failures — connection refused, or globals not (yet) advertised by
//! a compositor that may still be registering them — are retried for
//! a bounded window before the source concedes to the logind
//! fallback; only a notifier advertised below version 2 is a
//! definitive failure that concedes immediately.
//!
//! Known accounting edge: if the compositor connection drops
//! mid-AFK, the open interval is sealed and the remainder of the same
//! AFK period becomes a second interval after reconnect, so the
//! cutscene grace is forgiven twice for one natural absence. This
//! under-bills idle (over-credits interactive time) by at most
//! grace + backoff + onset per compositor restart — rare, and biased
//! in the user's favour, so it is accepted rather than special-cased.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::{oneshot, watch};
use tracing::{debug, info, instrument, warn};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::{
    self, ExtIdleNotifierV1,
};

use crate::idle::IdleTracker;

/// How long input must be absent before the compositor reports idle,
/// in milliseconds.
///
/// Deliberately small and fixed: this only defines idle *onset* — the
/// role logind's own hint threshold used to play. Cutscene-grace
/// forgiveness stays with the consumer
/// ([`IdleTracker::billable_idle_seconds_since`]); passing the
/// configured grace here instead would forgive it twice, once in the
/// compositor and again per interval in the billing math. The cost of
/// the onset is that every recorded interval is short by up to this
/// much, which is noise against the 300-second default grace — and a
/// fixed value means the notification object never needs destroying
/// and recreating when the grace setting changes.
const IDLE_ONSET_MS: u32 = 10_000;

/// Reconnect backoff bounds for a lost compositor connection.
const RECONNECT_BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// How long the first connection attempt keeps retrying transient
/// failures before conceding to the logind fallback. Covers the
/// compositor-still-starting race at login without leaving a
/// genuinely non-Wayland system sourceless for more than a minute.
const FIRST_CONNECT_WINDOW: Duration = Duration::from_secs(60);

/// Async-side ceiling on the whole setup handshake. The worker's own
/// deadline only fires *between* attempts — a socket that accepts but
/// is never serviced (the session's socket-holder hasn't exec'd the
/// compositor yet, or the compositor is mid-crash-restart) wedges a
/// roundtrip indefinitely with no timeout anywhere in the pure-Rust
/// client stack. When this fires the async side concedes to logind;
/// the worker's later `send` on the dropped channel tells it nobody
/// is listening and it exits without ever dispatching. Comfortably
/// above the worker's own worst case (~63 s of backoff sleeps).
const SETUP_WATCHDOG: Duration = Duration::from_secs(90);

/// Why a connection attempt failed, split by whether trying again
/// could plausibly change the answer.
enum SetupError {
    /// Socket absent / refused / dropped mid-setup — or a global not
    /// (yet) advertised. A compositor still registering its globals
    /// at session start is indistinguishable from one that lacks
    /// them, so absence is never treated as a final answer; the
    /// bounded first-connect window is what turns "still absent"
    /// into giving up.
    Transient(anyhow::Error),
    /// The compositor advertised `ext_idle_notifier_v1` below
    /// version 2. A global's version is fixed at registration, so
    /// retrying would ask the same compositor the same question.
    Definitive(anyhow::Error),
}

impl SetupError {
    fn into_inner(self) -> anyhow::Error {
        match self {
            Self::Transient(e) | Self::Definitive(e) => e,
        }
    }
}

/// Map a notification event to the tracker transition it implies.
/// Unknown events (future protocol revisions) map to `None`.
fn event_to_idle(event: &ext_idle_notification_v1::Event) -> Option<bool> {
    match event {
        ext_idle_notification_v1::Event::Idled => Some(true),
        ext_idle_notification_v1::Event::Resumed => Some(false),
        _ => None,
    }
}

/// Apply a notification event to the shared tracker.
fn handle_notification_event(tracker: &IdleTracker, event: &ext_idle_notification_v1::Event) {
    if let Some(idle) = event_to_idle(event) {
        debug!(idle, "ext-idle-notify transition");
        tracker.set_idle(idle);
    }
}

/// Per-connection dispatch state for the worker thread.
struct WorkerState {
    tracker: Arc<IdleTracker>,
    /// `(name, version)` of the advertised `ext_idle_notifier_v1`.
    notifier_global: Option<(u32, u32)>,
    /// Registry name of the first advertised `wl_seat`.
    seat_global: Option<u32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WorkerState {
    fn event(
        state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "ext_idle_notifier_v1" => state.notifier_global = Some((name, version)),
                "wl_seat" if state.seat_global.is_none() => state.seat_global = Some(name),
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WorkerState {
    fn event(
        _state: &mut Self,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The seat is only a parameter to the idle request; its
        // capability/name events are irrelevant here.
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for WorkerState {
    fn event(
        _state: &mut Self,
        _notifier: &ExtIdleNotifierV1,
        _event: ext_idle_notifier_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // ext_idle_notifier_v1 defines no events.
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        _notification: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        handle_notification_event(&state.tracker, &event);
    }
}

/// One live compositor connection with its idle notification armed.
struct IdleConnection {
    queue: wayland_client::EventQueue<WorkerState>,
    state: WorkerState,
}

/// Open a Wayland connection: `WAYLAND_DISPLAY` when the environment
/// has it, otherwise the default `wayland-0` socket under
/// `XDG_RUNTIME_DIR` (the packaged-daemon path — see the module doc).
fn open_connection() -> Result<Connection> {
    let env_err = match Connection::connect_to_env() {
        Ok(conn) => return Ok(conn),
        Err(e) => e,
    };
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").with_context(|| {
        format!("connect via env failed ({env_err}) and XDG_RUNTIME_DIR is unset")
    })?;
    let socket_path = std::path::Path::new(&runtime_dir).join("wayland-0");
    let stream = std::os::unix::net::UnixStream::connect(&socket_path).with_context(|| {
        format!(
            "connect via env failed ({env_err}) and so did the default socket {}",
            socket_path.display()
        )
    })?;
    Connection::from_socket(stream).context("initiate Wayland connection on default socket")
}

/// Connect to the compositor, verify it speaks
/// `ext_idle_notifier_v1` at version ≥ 2, and arm an input-idle
/// notification.
///
/// The second roundtrip surfaces any protocol error the compositor
/// raised for the binds or the request, so a returned `Ok` means the
/// notification is genuinely armed.
fn connect(tracker: Arc<IdleTracker>) -> Result<IdleConnection, SetupError> {
    let conn = open_connection().map_err(SetupError::Transient)?;
    let display = conn.display();
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let registry = display.get_registry(&qh, ());

    let mut state = WorkerState {
        tracker,
        notifier_global: None,
        seat_global: None,
    };
    queue
        .roundtrip(&mut state)
        .context("initial registry roundtrip")
        .map_err(SetupError::Transient)?;

    let (notifier_name, notifier_version) = state
        .notifier_global
        .context("compositor does not advertise ext_idle_notifier_v1")
        .map_err(SetupError::Transient)?;
    if notifier_version < 2 {
        return Err(SetupError::Definitive(anyhow::anyhow!(
            "ext_idle_notifier_v1 advertised at version {notifier_version}, \
             need >= 2 for input-idle (inhibitor-ignoring) notifications"
        )));
    }
    let seat_name = state
        .seat_global
        .context("no wl_seat advertised")
        .map_err(SetupError::Transient)?;

    // Seat version 1 suffices — it is only a parameter to the idle
    // request; none of its later-version events are consumed.
    let seat: wl_seat::WlSeat = registry.bind(seat_name, 1, &qh, ());
    let notifier: ExtIdleNotifierV1 = registry.bind(notifier_name, 2, &qh, ());
    let _notification = notifier.get_input_idle_notification(IDLE_ONSET_MS, &seat, &qh, ());
    queue
        .roundtrip(&mut state)
        .context("roundtrip after arming idle notification")
        .map_err(SetupError::Transient)?;

    Ok(IdleConnection { queue, state })
}

/// Worker-thread body: dispatch events forever, rebuilding the
/// connection with capped exponential backoff when the compositor
/// drops it. Reports the outcome of the *first* connection attempt
/// through `first_setup` so the async side can fall back to logind.
#[allow(
    clippy::needless_pass_by_value,
    reason = "thread entry point; the worker owns its tracker handle for the process lifetime"
)]
fn worker(tracker: Arc<IdleTracker>, first_setup: oneshot::Sender<Result<()>>) {
    // First connection: retry transient failures (compositor still
    // starting at login) within the window; concede immediately on a
    // definitive answer from a live compositor.
    let deadline = std::time::Instant::now() + FIRST_CONNECT_WINDOW;
    let mut backoff = RECONNECT_BACKOFF_INITIAL;
    let mut conn = loop {
        match connect(Arc::clone(&tracker)) {
            Ok(c) => {
                if first_setup.send(Ok(())).is_err() {
                    // The async side stopped listening — shutdown, or
                    // the setup watchdog already conceded to logind.
                    // Dispatching now would add a second writer to the
                    // tracker, so exit instead.
                    return;
                }
                break c;
            }
            Err(SetupError::Definitive(e)) => {
                let _ = first_setup.send(Err(e));
                return;
            }
            Err(SetupError::Transient(e)) => {
                if std::time::Instant::now() >= deadline {
                    let _ = first_setup.send(Err(e.context(format!(
                        "no Wayland compositor reachable within {}s",
                        FIRST_CONNECT_WINDOW.as_secs()
                    ))));
                    return;
                }
                debug!(error = %e, "wayland connect attempt failed; retrying");
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
            }
        }
    };

    let mut backoff = RECONNECT_BACKOFF_INITIAL;
    loop {
        match conn.queue.blocking_dispatch(&mut conn.state) {
            Ok(_) => backoff = RECONNECT_BACKOFF_INITIAL,
            Err(e) => {
                // With the connection gone the resume edge can never be
                // observed; seal any open interval rather than letting
                // it grow unboundedly.
                tracker.set_idle(false);

                let retryable = matches!(
                    &e,
                    wayland_client::DispatchError::Backend(
                        wayland_client::backend::WaylandError::Io(_)
                    )
                );
                if !retryable {
                    // A protocol error means a bug in our requests;
                    // retrying would just repeat it.
                    warn!(error = %e, "wayland idle source hit a protocol error; idle tracking stopped");
                    return;
                }
                warn!(error = %e, "wayland connection lost; reconnecting");
                loop {
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
                    match connect(Arc::clone(&tracker)) {
                        Ok(c) => {
                            info!("wayland idle source reconnected");
                            conn = c;
                            break;
                        }
                        Err(e) => {
                            debug!(error = %e.into_inner(), "wayland reconnect failed");
                        }
                    }
                }
            }
        }
    }
}

/// Drive `tracker` from the best available idle source until
/// `shutdown` fires.
///
/// Tries `ext-idle-notify-v1` version 2 first — via `WAYLAND_DISPLAY`
/// or the default `wayland-0` socket, with transient failures retried
/// for [`FIRST_CONNECT_WINDOW`]. When no usable Wayland session
/// exists (X11, no compositor socket, compositor too old) it falls
/// back to the logind `IdleHint` watcher in [`crate::idle`]. Exactly
/// one source ever drives the tracker: `IdleTracker::set_idle` is
/// edge-triggered, so two concurrent sources would race on the open
/// interval and mis-seal it.
///
/// The Wayland worker thread is deliberately detached: it parks in
/// `blocking_dispatch`, which nothing can interrupt cheaply, and it
/// holds nothing needing teardown — the process exit that follows
/// this function's return reaps it.
#[instrument(name = "idle_source", skip_all)]
pub async fn run_watcher(
    tracker: Arc<IdleTracker>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let (setup_tx, setup_rx) = oneshot::channel();
    {
        let tracker = Arc::clone(&tracker);
        std::thread::Builder::new()
            .name("idle-wayland".into())
            .spawn(move || worker(tracker, setup_tx))
            .context("spawn wayland idle worker thread")?;
    }

    // The handshake can take up to FIRST_CONNECT_WINDOW while the
    // worker retries; a shutdown arriving in that window must not
    // block the daemon's exit path (the final backup and session
    // close wait on this task's join). The watchdog arm covers a
    // worker wedged inside an unserviced roundtrip (see
    // SETUP_WATCHDOG). Both non-reply arms drop `setup_rx`, which is
    // what guarantees a late-succeeding worker sees a closed channel
    // and exits instead of becoming a second tracker writer.
    let setup = tokio::select! {
        biased;
        _ = shutdown.changed() => return Ok(()),
        result = setup_rx => match result {
            Ok(r) => r,
            Err(_) => Err(anyhow::anyhow!(
                "wayland idle worker exited before reporting its setup result"
            )),
        },
        () = tokio::time::sleep(SETUP_WATCHDOG) => Err(anyhow::anyhow!(
            "wayland idle setup unresponsive after {}s (socket accepted but never serviced?)",
            SETUP_WATCHDOG.as_secs()
        )),
    };
    match setup {
        Ok(()) => {
            info!(
                onset_ms = IDLE_ONSET_MS,
                "wayland idle source started (ext-idle-notify-v1 v2, input-based)"
            );
            // The worker owns everything from here; wait so the
            // daemon's shutdown join discipline holds.
            let _ = shutdown.changed().await;
            Ok(())
        }
        Err(e) => {
            info!(error = %e, "wayland idle source unavailable; falling back to logind IdleHint");
            crate::idle::run_watcher(tracker, shutdown).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idled_event_opens_interval_and_resumed_seals_it() {
        let t = IdleTracker::new();
        handle_notification_event(&t, &ext_idle_notification_v1::Event::Idled);
        assert!(t.is_idle());
        handle_notification_event(&t, &ext_idle_notification_v1::Event::Resumed);
        assert!(!t.is_idle());
        assert_eq!(t.closed_intervals_count(), 1);
    }

    #[test]
    fn duplicate_idled_events_do_not_open_a_second_interval() {
        let t = IdleTracker::new();
        handle_notification_event(&t, &ext_idle_notification_v1::Event::Idled);
        handle_notification_event(&t, &ext_idle_notification_v1::Event::Idled);
        handle_notification_event(&t, &ext_idle_notification_v1::Event::Resumed);
        assert_eq!(t.closed_intervals_count(), 1);
        assert!(!t.is_idle());
    }
}
