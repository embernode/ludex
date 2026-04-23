//! `net.ludex.Tracker1` — the public D-Bus API.
//!
//! This is what the Tauri GUI (and anything else that wants to
//! observe ludex) connects to. Distinct from the
//! `org.kde.ludex.Tracker1` service owned by the KWin source, which
//! is internal glue for the compositor callback and is not a stable
//! API surface.
//!
//! # Shape
//!
//! ```text
//! bus   : net.ludex.Tracker1 (session bus)
//! path  : /net/ludex/Tracker1
//! iface : net.ludex.Tracker1
//! ```
//!
//! Methods return plain values; signals notify the client of session
//! lifecycle events so the GUI can refresh without polling. DTOs are
//! serde-serializable structs; the zbus macro derives the
//! matching D-Bus struct signatures via `zvariant::Type`.
//!
//! # Error handling
//!
//! SQL errors and invariant violations are mapped to
//! `zbus::fdo::Error::Failed(message)`. The message is human-
//! readable; GUI code should surface it verbatim in a toast and log
//! it for troubleshooting.

// The `zbus::interface` macro generates protocol-glue items that do
// not carry our /// comments, so `missing_docs` fires spuriously on
// the macro output. Scope the relaxation to this module only.
#![allow(
    missing_docs,
    reason = "zbus::interface emits helper items without doc comments"
)]

use std::sync::Arc;

use ludex_core::repo::{DEFAULT_GPU_MEMORY_THRESHOLD_BYTES, GPU_MEMORY_THRESHOLD_BYTES};
use ludex_core::{Database, LauncherType, Session};
pub use ludex_dbus_types::{
    ApplicationSummary, DailyPlaytime, SessionSummary, OBJECT_PATH, SERVICE_NAME,
};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, instrument, warn};
use zbus::object_server::SignalEmitter;
use zbus::Connection;

use crate::session_manager::SharedBlocklist;

/// A session-lifecycle notification the [`SessionManager`] hands to
/// the D-Bus layer. The notifier task translates these into
/// `org.freedesktop.DBus.Signal` emissions on the session bus.
#[derive(Debug, Clone, Copy)]
pub enum TrackerNotification {
    /// An application row was created for a newly-seen game.
    ApplicationAdded {
        /// Application id.
        application_id: i64,
    },
    /// A session opened (`GameEvent::Started` accepted).
    SessionStarted {
        /// Application id.
        application_id: i64,
    },
    /// A session closed (`GameEvent::Stopped`, graceful shutdown, or
    /// pidfd-observed exit).
    SessionEnded {
        /// Application id.
        application_id: i64,
        /// Full-runtime seconds of the session that just ended.
        full_runtime_seconds: i64,
        /// Interactive-runtime seconds of that session.
        interactive_runtime_seconds: i64,
    },
}

/// The D-Bus interface object served at [`OBJECT_PATH`].
pub struct Tracker {
    db: Arc<Database>,
    blocklist: SharedBlocklist,
}

impl Tracker {
    /// Construct a tracker bound to the given database handle and
    /// the shared blocklist the session manager also watches.
    /// Block/unblock RPCs mutate both the DB row and this handle, so
    /// changes take effect on the very next `Started` event.
    #[must_use]
    pub fn new(db: Arc<Database>, blocklist: SharedBlocklist) -> Self {
        Self { db, blocklist }
    }
}

#[zbus::interface(name = "net.ludex.Tracker1")]
impl Tracker {
    /// List every tracked application, most-recently-played first.
    async fn list_applications(&self) -> zbus::fdo::Result<Vec<ApplicationSummary>> {
        let apps = self
            .db
            .applications()
            .list()
            .await
            .map_err(|e| into_fdo(&e))?;
        Ok(apps.into_iter().map(application_summary_from).collect())
    }

    /// Return one application by primary-key id. Returns an empty
    /// list when no such id exists; D-Bus lacks a clean "optional"
    /// primitive, so we emulate with a 0-or-1-element list.
    async fn get_application(&self, id: i64) -> zbus::fdo::Result<Vec<ApplicationSummary>> {
        let app = self
            .db
            .applications()
            .find_by_id(id)
            .await
            .map_err(|e| into_fdo(&e))?;
        Ok(app.map(application_summary_from).into_iter().collect())
    }

    /// The most recent `limit` sessions across every application
    /// (joined to the application's product name). `limit` is
    /// clamped to `[1, 1000]`.
    async fn list_recent_sessions(&self, limit: u32) -> zbus::fdo::Result<Vec<SessionSummary>> {
        let limit = limit.clamp(1, 1000);
        let rows = self
            .db
            .sessions()
            .list_recent_with_app(limit)
            .await
            .map_err(|e| into_fdo(&e))?;
        Ok(rows
            .into_iter()
            .map(|row| SessionSummary {
                id: row.id,
                application_id: row.application_id,
                product_name: row.product_name,
                started_at: format_datetime(row.started_at),
                ended_at: row.ended_at.map(format_datetime).unwrap_or_default(),
                full_runtime_seconds: row.full_runtime_seconds,
                interactive_runtime_seconds: row.interactive_runtime_seconds,
                exit_reason: row.exit_reason.map(|r| r.to_string()).unwrap_or_default(),
            })
            .collect())
    }

    /// Per-day aggregate runtime for the last `days` days (clamped to
    /// `[1, 3650]`). One row per day that has at least one session;
    /// days with no activity are omitted, so the GUI fills gaps with
    /// zeros where the chart needs a continuous range.
    async fn list_daily_playtime(&self, days: u32) -> zbus::fdo::Result<Vec<DailyPlaytime>> {
        let days = days.clamp(1, 3650);
        let cutoff = OffsetDateTime::now_utc() - Duration::days(i64::from(days));
        let rows = self
            .db
            .sessions()
            .daily_playtime_since(cutoff)
            .await
            .map_err(|e| into_fdo(&e))?;
        Ok(rows
            .into_iter()
            .map(|r| DailyPlaytime {
                date: r.date,
                full_runtime_seconds: r.full_runtime_seconds,
                interactive_runtime_seconds: r.interactive_runtime_seconds,
                session_count: r.session_count,
            })
            .collect())
    }

    /// Sessions for a single application, most-recent first.
    async fn list_sessions_for_application(
        &self,
        application_id: i64,
        limit: u32,
    ) -> zbus::fdo::Result<Vec<SessionSummary>> {
        let limit = limit.clamp(1, 1000);
        let app = self
            .db
            .applications()
            .find_by_id(application_id)
            .await
            .map_err(|e| into_fdo(&e))?;
        let product_name = app
            .as_ref()
            .map(|a| a.product_name.clone())
            .unwrap_or_default();
        let sessions: Vec<Session> = self
            .db
            .sessions()
            .list_for_application(application_id, limit)
            .await
            .map_err(|e| into_fdo(&e))?;
        Ok(sessions
            .iter()
            .map(|s| session_summary_for(application_id, product_name.clone(), s))
            .collect())
    }

    /// Primary keys of every application currently present in the
    /// `blocked_applications` table. The GUI cross-references these
    /// against `ListApplications` output to display a blocked-state
    /// toggle per row.
    async fn list_blocked_application_ids(&self) -> zbus::fdo::Result<Vec<i64>> {
        let keys = self.db.blocked().list().await.map_err(|e| into_fdo(&e))?;
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // There's typically one blocked entry at most; looking up
        // ids individually keeps the query trivial. If the blocklist
        // grows large, a JOIN against applications would be the
        // obvious follow-up.
        let mut ids = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(app) = self
                .db
                .applications()
                .find_by_key(&key)
                .await
                .map_err(|e| into_fdo(&e))?
            {
                ids.push(app.id);
            }
        }
        Ok(ids)
    }

    /// Block the application with primary-key `id`. Updates both the
    /// `blocked_applications` table and the shared in-memory set
    /// consulted by the session manager, so the next `Started` for
    /// this key will be dropped. Blocking an id that doesn't exist
    /// returns `NotFound`.
    async fn block_application(&self, id: i64) -> zbus::fdo::Result<()> {
        let app = self
            .db
            .applications()
            .find_by_id(id)
            .await
            .map_err(|e| into_fdo(&e))?
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("no application with id {id}")))?;
        let key = ludex_core::GameKey::new(app.launcher_type, app.launcher_id);
        self.db
            .blocked()
            .insert(&key, OffsetDateTime::now_utc())
            .await
            .map_err(|e| into_fdo(&e))?;
        self.blocklist.write().await.insert(key);
        info!(application_id = id, "application blocked");
        Ok(())
    }

    /// Unblock the application with primary-key `id`. Safe to call
    /// on an id that isn't blocked — the call returns Ok and
    /// nothing changes.
    async fn unblock_application(&self, id: i64) -> zbus::fdo::Result<()> {
        let Some(app) = self
            .db
            .applications()
            .find_by_id(id)
            .await
            .map_err(|e| into_fdo(&e))?
        else {
            return Err(zbus::fdo::Error::Failed(format!(
                "no application with id {id}"
            )));
        };
        let key = ludex_core::GameKey::new(app.launcher_type, app.launcher_id);
        self.db
            .blocked()
            .remove(&key)
            .await
            .map_err(|e| into_fdo(&e))?;
        self.blocklist.write().await.remove(&key);
        info!(application_id = id, "application unblocked");
        Ok(())
    }

    /// Per-process GPU memory threshold the foreground-window
    /// fallback uses to accept a non-fullscreen window as a game.
    /// Reports the stored value when present, otherwise the compiled-
    /// in default.
    async fn get_gpu_memory_threshold_bytes(&self) -> zbus::fdo::Result<u64> {
        self.db
            .settings()
            .get_u64(
                GPU_MEMORY_THRESHOLD_BYTES,
                DEFAULT_GPU_MEMORY_THRESHOLD_BYTES,
            )
            .await
            .map_err(|e| into_fdo(&e))
    }

    /// Update the GPU memory threshold setting. Takes effect on the
    /// next daemon start — live reload is a follow-up that needs a
    /// shared `Arc<RwLock<GateConfig>>` the gate reads from. The GUI
    /// should surface the "restart required" state to the user.
    async fn set_gpu_memory_threshold_bytes(&self, bytes: u64) -> zbus::fdo::Result<()> {
        self.db
            .settings()
            .set_u64(GPU_MEMORY_THRESHOLD_BYTES, bytes)
            .await
            .map_err(|e| into_fdo(&e))?;
        info!(gpu_memory_threshold_bytes = bytes, "setting updated");
        Ok(())
    }

    /// Fired when a fresh application row was inserted into the
    /// database. Clients that maintain an in-memory list of
    /// applications should re-read `ListApplications`.
    #[zbus(signal)]
    async fn application_added(
        emitter: &SignalEmitter<'_>,
        application_id: i64,
    ) -> zbus::Result<()>;

    /// Fired when a session opens for `application_id`.
    #[zbus(signal)]
    async fn session_started(emitter: &SignalEmitter<'_>, application_id: i64) -> zbus::Result<()>;

    /// Fired when a session closes.
    #[zbus(signal)]
    async fn session_ended(
        emitter: &SignalEmitter<'_>,
        application_id: i64,
        full_runtime_seconds: i64,
        interactive_runtime_seconds: i64,
    ) -> zbus::Result<()>;
}

/// Convert a `ludex_core::Error` into the `zbus::fdo::Error::Failed`
/// variant the GUI can display to the user without leaking a
/// stringly-typed error tag.
fn into_fdo(e: &ludex_core::Error) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(e.to_string())
}

fn format_datetime(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn application_summary_from(app: ludex_core::Application) -> ApplicationSummary {
    ApplicationSummary {
        id: app.id,
        launcher_type: launcher_type_string(app.launcher_type),
        launcher_id: app.launcher_id,
        product_name: app.product_name,
        publisher: app.publisher.unwrap_or_default(),
        total_full_seconds: app.stat_total_full,
        total_interactive_seconds: app.stat_total_interactive,
        run_count: app.stat_run_count,
        last_played_at: app.last_played_at.map(format_datetime).unwrap_or_default(),
    }
}

fn session_summary_for(application_id: i64, product_name: String, s: &Session) -> SessionSummary {
    SessionSummary {
        id: s.id,
        application_id,
        product_name,
        started_at: format_datetime(s.started_at),
        ended_at: s.ended_at.map(format_datetime).unwrap_or_default(),
        full_runtime_seconds: s.full_runtime_seconds,
        interactive_runtime_seconds: s.interactive_runtime_seconds,
        exit_reason: s.exit_reason.map(|r| r.to_string()).unwrap_or_default(),
    }
}

fn launcher_type_string(lt: LauncherType) -> String {
    lt.to_string()
}

/// Register the Tracker service on a fresh session-bus connection.
///
/// The daemon already owns a *separate* session-bus connection for
/// the KWin callback; this one is purposely independent so the public
/// API's lifecycle is not tangled with the compositor integration.
pub async fn serve(db: Arc<Database>, blocklist: SharedBlocklist) -> anyhow::Result<Connection> {
    let tracker = Tracker::new(db, blocklist);
    let conn = zbus::connection::Builder::session()?
        .name(SERVICE_NAME)?
        .serve_at(OBJECT_PATH, tracker)?
        .build()
        .await?;
    info!(
        service = SERVICE_NAME,
        path = OBJECT_PATH,
        "public D-Bus API registered"
    );
    Ok(conn)
}

/// Background task that translates [`TrackerNotification`]s into
/// D-Bus signals. Runs until the channel closes or `shutdown` fires.
#[instrument(name = "tracker_notifier", skip_all)]
pub async fn run_notifier(
    conn: Connection,
    mut rx: mpsc::Receiver<TrackerNotification>,
    mut shutdown: watch::Receiver<bool>,
) {
    let iface_ref = match conn
        .object_server()
        .interface::<_, Tracker>(OBJECT_PATH)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "tracker interface missing from object server; notifier exiting");
            return;
        }
    };
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            maybe = rx.recv() => {
                let Some(notif) = maybe else {
                    debug!("notification channel closed; notifier exiting");
                    break;
                };
                emit(&iface_ref, notif).await;
            }
        }
    }
}

async fn emit(
    iface_ref: &zbus::object_server::InterfaceRef<Tracker>,
    notification: TrackerNotification,
) {
    let emitter = iface_ref.signal_emitter();
    let result = match notification {
        TrackerNotification::ApplicationAdded { application_id } => {
            Tracker::application_added(emitter, application_id).await
        }
        TrackerNotification::SessionStarted { application_id } => {
            Tracker::session_started(emitter, application_id).await
        }
        TrackerNotification::SessionEnded {
            application_id,
            full_runtime_seconds,
            interactive_runtime_seconds,
        } => {
            Tracker::session_ended(
                emitter,
                application_id,
                full_runtime_seconds,
                interactive_runtime_seconds,
            )
            .await
        }
    };
    if let Err(e) = result {
        warn!(error = %e, ?notification, "failed to emit D-Bus signal");
    }
}
