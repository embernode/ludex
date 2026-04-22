//! D-Bus bridge between the Tauri host and the ludex daemon.
//!
//! Consumes [`net.ludex.Tracker1`](../../crates/ludex-daemon/src/dbus.rs)
//! from the daemon and re-exposes it to the Svelte frontend through
//! two narrow surfaces:
//!
//! * `#[tauri::command]` async functions invoked from JavaScript via
//!   `@tauri-apps/api/core.invoke`. These are synchronous-ish RPCs —
//!   the UI asks for the list of applications, the daemon answers.
//! * Tauri events emitted whenever the daemon fires a D-Bus signal
//!   (application added, session started, session ended). The
//!   frontend subscribes with `@tauri-apps/api/event.listen` and
//!   re-renders reactively.
//!
//! The D-Bus connection is lazy: we open the session bus on the
//! first call and reuse it afterwards. A daemon that isn't running
//! surfaces as a clean error string the UI can show in a toast
//! rather than a panic.

// The `zbus::proxy` macro synthesises helper items (signal-argument
// structs, receive_* methods) whose public docs aren't practical to
// hand-write. Scope the relaxation to this module only.
#![allow(
    missing_docs,
    reason = "zbus::proxy emits helper items without doc comments"
)]

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::OnceCell;
use zbus::zvariant::Type;

/// Tauri event name emitted on every `ApplicationAdded` signal.
pub(crate) const EVENT_APPLICATION_ADDED: &str = "ludex:application-added";
/// Tauri event name emitted on every `SessionStarted` signal.
pub(crate) const EVENT_SESSION_STARTED: &str = "ludex:session-started";
/// Tauri event name emitted on every `SessionEnded` signal.
pub(crate) const EVENT_SESSION_ENDED: &str = "ludex:session-ended";

/// GUI-shaped application row. Shape must stay byte-identical to
/// `ludex_daemon::dbus::ApplicationSummary` or the zbus decoder
/// will reject replies. They are duplicated rather than shared
/// through a crate because pulling `ludex-daemon` into the Tauri
/// host would drag the entire detection/sqlite/KWin stack into the
/// GUI binary for nothing.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ApplicationSummary {
    pub(crate) id: i64,
    pub(crate) launcher_type: String,
    pub(crate) launcher_id: String,
    pub(crate) product_name: String,
    pub(crate) publisher: String,
    pub(crate) total_full_seconds: i64,
    pub(crate) total_interactive_seconds: i64,
    pub(crate) run_count: i64,
    pub(crate) last_played_at: String,
}

/// GUI-shaped session row; mirrors
/// `ludex_daemon::dbus::SessionSummary`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct SessionSummary {
    pub(crate) id: i64,
    pub(crate) application_id: i64,
    pub(crate) product_name: String,
    pub(crate) started_at: String,
    pub(crate) ended_at: String,
    pub(crate) full_runtime_seconds: i64,
    pub(crate) interactive_runtime_seconds: i64,
    pub(crate) exit_reason: String,
}

/// Payload for the `ludex:session-ended` Tauri event.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionEndedPayload {
    pub(crate) application_id: i64,
    pub(crate) full_runtime_seconds: i64,
    pub(crate) interactive_runtime_seconds: i64,
}

/// Minimal zbus proxy for the daemon's `net.ludex.Tracker1`
/// interface. Only includes the methods and signals the GUI
/// currently consumes; more can be added as views expand.
#[zbus::proxy(
    interface = "net.ludex.Tracker1",
    default_service = "net.ludex.Tracker1",
    default_path = "/net/ludex/Tracker1"
)]
pub(crate) trait Tracker {
    fn list_applications(&self) -> zbus::Result<Vec<ApplicationSummary>>;
    fn get_application(&self, id: i64) -> zbus::Result<Vec<ApplicationSummary>>;
    fn list_recent_sessions(&self, limit: u32) -> zbus::Result<Vec<SessionSummary>>;
    fn list_sessions_for_application(
        &self,
        application_id: i64,
        limit: u32,
    ) -> zbus::Result<Vec<SessionSummary>>;

    #[zbus(signal)]
    fn application_added(&self, application_id: i64) -> zbus::Result<()>;
    #[zbus(signal)]
    fn session_started(&self, application_id: i64) -> zbus::Result<()>;
    #[zbus(signal)]
    fn session_ended(
        &self,
        application_id: i64,
        full_runtime_seconds: i64,
        interactive_runtime_seconds: i64,
    ) -> zbus::Result<()>;
}

/// Shared state managed by Tauri. Hands out a proxy to every command
/// that needs one; the underlying connection is opened once on
/// first use.
pub(crate) struct TrackerBridge {
    connection: OnceCell<zbus::Connection>,
}

impl TrackerBridge {
    /// Construct an empty bridge. The D-Bus connection is not
    /// opened until the first command call, so GUI startup stays
    /// responsive even with the daemon down.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            connection: OnceCell::new(),
        }
    }

    /// Return a proxy bound to the session bus. On first call,
    /// connects to the bus; subsequent calls reuse the cached
    /// connection.
    pub(crate) async fn proxy(&self) -> Result<TrackerProxy<'_>, zbus::Error> {
        let conn = self
            .connection
            .get_or_try_init(zbus::Connection::session)
            .await?;
        TrackerProxy::new(conn).await
    }
}

impl Default for TrackerBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared-state alias used in Tauri command signatures.
pub(crate) type BridgeState<'r> = State<'r, Arc<TrackerBridge>>;

/// `invoke('list_applications')` returns every tracked application,
/// most-recently-played first.
#[tauri::command]
pub(crate) async fn list_applications(
    bridge: BridgeState<'_>,
) -> Result<Vec<ApplicationSummary>, String> {
    let proxy = bridge.proxy().await.map_err(friendly)?;
    proxy.list_applications().await.map_err(friendly)
}

/// `invoke('get_application', { id })` returns one application by
/// id, or an empty array when no such id exists.
#[tauri::command]
pub(crate) async fn get_application(
    bridge: BridgeState<'_>,
    id: i64,
) -> Result<Vec<ApplicationSummary>, String> {
    let proxy = bridge.proxy().await.map_err(friendly)?;
    proxy.get_application(id).await.map_err(friendly)
}

/// `invoke('list_recent_sessions', { limit })` returns the N most
/// recent sessions across every application.
#[tauri::command]
pub(crate) async fn list_recent_sessions(
    bridge: BridgeState<'_>,
    limit: u32,
) -> Result<Vec<SessionSummary>, String> {
    let proxy = bridge.proxy().await.map_err(friendly)?;
    proxy.list_recent_sessions(limit).await.map_err(friendly)
}

/// `invoke('list_sessions_for_application', { applicationId, limit })`.
#[tauri::command]
pub(crate) async fn list_sessions_for_application(
    bridge: BridgeState<'_>,
    application_id: i64,
    limit: u32,
) -> Result<Vec<SessionSummary>, String> {
    let proxy = bridge.proxy().await.map_err(friendly)?;
    proxy
        .list_sessions_for_application(application_id, limit)
        .await
        .map_err(friendly)
}

/// Subscribe to the daemon's D-Bus signals and re-emit them as
/// Tauri events so every window can react. Runs forever; spawn on
/// the Tauri async runtime at setup time.
///
/// A daemon that isn't running at GUI start isn't an error — zbus
/// still subscribes via match rules on the bus, and signals begin
/// flowing as soon as the daemon registers its service name.
/// Daemon *restarts* (well-known name changing owner) require
/// refreshing the subscription; we log and exit on stream closure
/// and leave reconnection logic for a later tranche.
pub(crate) async fn run_signal_forwarder(app: AppHandle, bridge: Arc<TrackerBridge>) {
    use futures_util::StreamExt as _;

    let proxy = match bridge.proxy().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "signal forwarder: could not build proxy; giving up");
            return;
        }
    };

    let mut added = match proxy.receive_application_added().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "receive_application_added failed; forwarder exiting");
            return;
        }
    };
    let mut started = match proxy.receive_session_started().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "receive_session_started failed; forwarder exiting");
            return;
        }
    };
    let mut ended = match proxy.receive_session_ended().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "receive_session_ended failed; forwarder exiting");
            return;
        }
    };

    loop {
        tokio::select! {
            Some(sig) = added.next() => {
                if let Ok(args) = sig.args() {
                    let _ = app.emit(EVENT_APPLICATION_ADDED, args.application_id);
                }
            }
            Some(sig) = started.next() => {
                if let Ok(args) = sig.args() {
                    let _ = app.emit(EVENT_SESSION_STARTED, args.application_id);
                }
            }
            Some(sig) = ended.next() => {
                if let Ok(args) = sig.args() {
                    let _ = app.emit(
                        EVENT_SESSION_ENDED,
                        SessionEndedPayload {
                            application_id: args.application_id,
                            full_runtime_seconds: args.full_runtime_seconds,
                            interactive_runtime_seconds: args.interactive_runtime_seconds,
                        },
                    );
                }
            }
            else => {
                tracing::info!("all D-Bus signal streams closed; forwarder exiting");
                break;
            }
        }
    }
}

/// Convert any error into the short, human-readable string Tauri
/// serializes to the frontend. `to_string` is usually enough;
/// we wrap here to keep a single place to refine messages later
/// (e.g. special-case `ServiceUnknown` into "ludex-daemon is not
/// running").
fn friendly(e: impl std::fmt::Display) -> String {
    e.to_string()
}
