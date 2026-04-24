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

pub(crate) use ludex_dbus_types::{
    ApplicationSummary, DailyPlaytime, SessionSummary, SERVICE_NAME,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::OnceCell;

/// Tauri event name emitted on every `ApplicationAdded` signal.
pub(crate) const EVENT_APPLICATION_ADDED: &str = "ludex:application-added";
/// Tauri event name emitted on every `SessionStarted` signal.
pub(crate) const EVENT_SESSION_STARTED: &str = "ludex:session-started";
/// Tauri event name emitted on every `SessionEnded` signal.
pub(crate) const EVENT_SESSION_ENDED: &str = "ludex:session-ended";
/// Tauri event emitted after the bridge rebuilds its D-Bus
/// subscription — either because `ludex-daemon` just came up, or
/// because it restarted. Signals to frontend pages that any local
/// data they cached may be stale. Not emitted on the first-ever
/// connect (the page's own `onMount` fetch handles that case).
pub(crate) const EVENT_DAEMON_RECONNECTED: &str = "ludex:daemon-reconnected";
/// Tauri event emitted after a successful `block_application` /
/// `unblock_application`. Listeners refresh so filtered views
/// (Games, Recent, Dashboard) reflect the new blocklist without
/// waiting for a session event. Emitted from the bridge rather
/// than as a D-Bus signal from the daemon because every block
/// path today goes through these Tauri commands — a future CLI
/// `ludex block` would need the daemon-side signal, but we don't
/// have one yet.
pub(crate) const EVENT_BLOCKLIST_CHANGED: &str = "ludex:blocklist-changed";

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
    fn list_daily_playtime(&self, days: u32) -> zbus::Result<Vec<DailyPlaytime>>;
    fn list_blocked_application_ids(&self) -> zbus::Result<Vec<i64>>;
    fn block_application(&self, id: i64) -> zbus::Result<()>;
    fn unblock_application(&self, id: i64) -> zbus::Result<()>;
    fn get_gpu_memory_threshold_bytes(&self) -> zbus::Result<u64>;
    fn set_gpu_memory_threshold_bytes(&self, bytes: u64) -> zbus::Result<()>;

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
        TrackerProxy::new(self.connection().await?).await
    }

    /// Borrow the cached session-bus connection, opening it on first
    /// call. Used by the signal forwarder to subscribe to
    /// `NameOwnerChanged` on the same connection as the tracker
    /// proxy.
    pub(crate) async fn connection(&self) -> Result<&zbus::Connection, zbus::Error> {
        self.connection
            .get_or_try_init(zbus::Connection::session)
            .await
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
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy.list_applications().await.map_err(|e| friendly(&e))
}

/// `invoke('get_application', { id })` returns one application by
/// id, or an empty array when no such id exists.
#[tauri::command]
pub(crate) async fn get_application(
    bridge: BridgeState<'_>,
    id: i64,
) -> Result<Vec<ApplicationSummary>, String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy.get_application(id).await.map_err(|e| friendly(&e))
}

/// `invoke('list_recent_sessions', { limit })` returns the N most
/// recent sessions across every application.
#[tauri::command]
pub(crate) async fn list_recent_sessions(
    bridge: BridgeState<'_>,
    limit: u32,
) -> Result<Vec<SessionSummary>, String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .list_recent_sessions(limit)
        .await
        .map_err(|e| friendly(&e))
}

/// `invoke('list_daily_playtime', { days })` returns one row per day
/// with activity, oldest first, over the last `days` days.
#[tauri::command]
pub(crate) async fn list_daily_playtime(
    bridge: BridgeState<'_>,
    days: u32,
) -> Result<Vec<DailyPlaytime>, String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .list_daily_playtime(days)
        .await
        .map_err(|e| friendly(&e))
}

/// `invoke('list_blocked_application_ids')` returns ids of every
/// application the user has blocked.
#[tauri::command]
pub(crate) async fn list_blocked_application_ids(
    bridge: BridgeState<'_>,
) -> Result<Vec<i64>, String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .list_blocked_application_ids()
        .await
        .map_err(|e| friendly(&e))
}

/// `invoke('block_application', { id })`. On success emits
/// `EVENT_BLOCKLIST_CHANGED` so every open page can refresh.
#[tauri::command]
pub(crate) async fn block_application(
    app: AppHandle,
    bridge: BridgeState<'_>,
    id: i64,
) -> Result<(), String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .block_application(id)
        .await
        .map_err(|e| friendly(&e))?;
    let _ = app.emit(EVENT_BLOCKLIST_CHANGED, ());
    Ok(())
}

/// `invoke('unblock_application', { id })`. On success emits
/// `EVENT_BLOCKLIST_CHANGED` so every open page can refresh.
#[tauri::command]
pub(crate) async fn unblock_application(
    app: AppHandle,
    bridge: BridgeState<'_>,
    id: i64,
) -> Result<(), String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .unblock_application(id)
        .await
        .map_err(|e| friendly(&e))?;
    let _ = app.emit(EVENT_BLOCKLIST_CHANGED, ());
    Ok(())
}

/// `invoke('get_gpu_memory_threshold_bytes')`.
#[tauri::command]
pub(crate) async fn get_gpu_memory_threshold_bytes(bridge: BridgeState<'_>) -> Result<u64, String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .get_gpu_memory_threshold_bytes()
        .await
        .map_err(|e| friendly(&e))
}

/// `invoke('set_gpu_memory_threshold_bytes', { bytes })`. Takes
/// effect at the next daemon restart; the GUI should surface that
/// to the user.
#[tauri::command]
pub(crate) async fn set_gpu_memory_threshold_bytes(
    bridge: BridgeState<'_>,
    bytes: u64,
) -> Result<(), String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .set_gpu_memory_threshold_bytes(bytes)
        .await
        .map_err(|e| friendly(&e))
}

/// `invoke('list_sessions_for_application', { applicationId, limit })`.
#[tauri::command]
pub(crate) async fn list_sessions_for_application(
    bridge: BridgeState<'_>,
    application_id: i64,
    limit: u32,
) -> Result<Vec<SessionSummary>, String> {
    let proxy = bridge.proxy().await.map_err(|e| friendly(&e))?;
    proxy
        .list_sessions_for_application(application_id, limit)
        .await
        .map_err(|e| friendly(&e))
}

/// Why a signal-forwarding session ended — controls whether the
/// outer loop rebuilds subscriptions or gives up.
enum SessionOutcome {
    /// The daemon's well-known name changed owner (restart, or it
    /// just came up). Rebuild subscriptions against the new owner.
    OwnerChanged,
    /// Every signal stream closed without an owner change. Treated
    /// as terminal: zbus tore down the connection and there's
    /// nothing to reconnect to here.
    StreamsClosed,
    /// We couldn't build the proxy or a subscription at all. The
    /// daemon may simply not be running yet — outer loop waits for
    /// the name to come up, then rebuilds.
    SetupFailed,
}

/// Subscribe to the daemon's D-Bus signals and re-emit them as
/// Tauri events so every window can react. Runs forever; spawn on
/// the Tauri async runtime at setup time.
///
/// A daemon that isn't running at GUI start is not an error — we
/// wait on `NameOwnerChanged` until it appears, then subscribe.
/// Daemon *restarts* (the well-known name changing owner) also
/// trigger a rebuild: zbus's match rules target the current owner,
/// so without this signals would silently stop flowing until the
/// user refreshed.
pub(crate) async fn run_signal_forwarder(app: AppHandle, bridge: Arc<TrackerBridge>) {
    // First successful session is the initial connect — the page's
    // own `onMount` refresh already covers it, so we don't fire a
    // spurious `daemon-reconnected` then. Every subsequent session
    // counts as a reconnect and the frontend refreshes to pick up
    // any state that changed while the daemon was down.
    let mut is_reconnect = false;
    loop {
        match run_signal_session(&app, &bridge, is_reconnect).await {
            SessionOutcome::OwnerChanged => {
                tracing::info!("ludex-daemon owner changed on the bus; rebuilding subscriptions");
                is_reconnect = true;
            }
            SessionOutcome::SetupFailed => {
                // Wait for the service to appear (or its owner to
                // change) before retrying. `wait_for_service` uses
                // the same NameOwnerChanged watcher; it simply
                // returns when the name is owned.
                if !wait_for_service(&bridge).await {
                    tracing::warn!(
                        "could not reach session bus to await daemon; forwarder exiting"
                    );
                    return;
                }
                is_reconnect = true;
            }
            SessionOutcome::StreamsClosed => {
                tracing::info!(
                    "D-Bus signal streams closed with no owner change; forwarder exiting"
                );
                return;
            }
        }
    }
}

/// One subscription lifetime. Returns when the subscription should
/// be rebuilt (or abandoned). `is_reconnect` is true after the
/// first successful session so the frontend can tell
/// "daemon just came back" from "daemon was here when we started".
async fn run_signal_session(
    app: &AppHandle,
    bridge: &TrackerBridge,
    is_reconnect: bool,
) -> SessionOutcome {
    use futures_util::StreamExt as _;

    let proxy = match bridge.proxy().await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(error = %e, "signal forwarder: could not build proxy");
            return SessionOutcome::SetupFailed;
        }
    };

    let mut added = match proxy.receive_application_added().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "receive_application_added failed");
            return SessionOutcome::SetupFailed;
        }
    };
    let mut started = match proxy.receive_session_started().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "receive_session_started failed");
            return SessionOutcome::SetupFailed;
        }
    };
    let mut ended = match proxy.receive_session_ended().await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "receive_session_ended failed");
            return SessionOutcome::SetupFailed;
        }
    };
    let Some(mut owner_changed) = subscribe_owner_changed(&proxy).await else {
        return SessionOutcome::SetupFailed;
    };

    // All four streams are live — tell frontend pages they can
    // safely re-fetch, so any data that changed while the daemon
    // was down (merges, restores, importers) flows into the UI
    // without the user having to hit Refresh.
    if is_reconnect {
        let _ = app.emit(EVENT_DAEMON_RECONNECTED, ());
    }

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
            Some(_) = owner_changed.next() => {
                return SessionOutcome::OwnerChanged;
            }
            else => {
                return SessionOutcome::StreamsClosed;
            }
        }
    }
}

/// Subscribe to `NameOwnerChanged` filtered to our service name.
/// Any element on the resulting stream indicates the owner
/// transitioned (to a new owner, to nothing, or from nothing); we
/// treat all of them as "rebuild".
async fn subscribe_owner_changed(
    proxy: &TrackerProxy<'_>,
) -> Option<zbus::fdo::NameOwnerChangedStream> {
    let dbus = match zbus::fdo::DBusProxy::new(proxy.inner().connection()).await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(error = %e, "could not build DBusProxy for NameOwnerChanged");
            return None;
        }
    };
    match dbus
        .receive_name_owner_changed_with_args(&[(0, SERVICE_NAME)])
        .await
    {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::debug!(error = %e, "receive_name_owner_changed_with_args failed");
            None
        }
    }
}

/// Block until `net.ludex.Tracker1` has an owner on the session
/// bus. Returns `false` only when the bus itself is unreachable —
/// in that case reconnection is hopeless from inside the GUI
/// process.
async fn wait_for_service(bridge: &TrackerBridge) -> bool {
    use futures_util::StreamExt as _;

    let conn = match bridge.connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "wait_for_service: session bus unavailable");
            return false;
        }
    };
    let dbus = match zbus::fdo::DBusProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "wait_for_service: could not build DBusProxy");
            return false;
        }
    };

    // Subscribe before the has-owner check so we never miss a
    // transition that happens between the two calls.
    let mut changes = match dbus
        .receive_name_owner_changed_with_args(&[(0, SERVICE_NAME)])
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "wait_for_service: could not watch NameOwnerChanged");
            return false;
        }
    };

    let service = match zbus::names::BusName::try_from(SERVICE_NAME) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "wait_for_service: SERVICE_NAME not a valid BusName");
            return false;
        }
    };
    if matches!(dbus.name_has_owner(service).await, Ok(true)) {
        return true;
    }

    while let Some(signal) = changes.next().await {
        if let Ok(args) = signal.args() {
            // `new_owner` populated means the name was claimed.
            // Ignore transitions that only clear the owner.
            if args.new_owner().is_some() {
                return true;
            }
        }
    }
    false
}

/// Convert a zbus error into a short, human-readable string for the
/// frontend to show verbatim.
///
/// `ServiceUnknown` in particular is a common-case failure — the
/// user opened the GUI without the daemon running — and must not
/// leak its D-Bus wire form into the UI. Other errors pass through
/// as-is; they're rare enough to be worth their full text.
fn friendly(e: &zbus::Error) -> String {
    if let zbus::Error::MethodError(name, _message, _reply) = e {
        if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" {
            return "ludex-daemon is not running. Start it with `ludex-daemon`, \
                or enable the systemd user service."
                .to_owned();
        }
    }
    e.to_string()
}
