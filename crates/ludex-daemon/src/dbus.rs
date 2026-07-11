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

use anyhow::Context;
use ludex_core::backup::{list_backups, prune_backups, snapshot_now};
use ludex_core::repo::{
    ALT_TAB_GRACE_SECONDS, BACKUP_INTERVAL_HOURS, BACKUP_RETENTION_COUNT,
    DEFAULT_ALT_TAB_GRACE_SECONDS, DEFAULT_BACKUP_INTERVAL_HOURS, DEFAULT_BACKUP_RETENTION_COUNT,
    DEFAULT_GPU_MEMORY_THRESHOLD_BYTES, DEFAULT_IDLE_GRACE_SECONDS,
    DEFAULT_PAUSE_WHEN_BACKGROUNDED, GPU_MEMORY_THRESHOLD_BYTES, IDLE_GRACE_SECONDS,
    PAUSE_WHEN_BACKGROUNDED,
};
use ludex_core::session_merge::{
    merge_adjacent_recent, merge_adjacent_session, DEFAULT_MERGE_GAP_SECONDS,
};
use ludex_core::{default_backup_dir, Database, LauncherType, Session};
pub use ludex_dbus_types::{
    ApplicationSummary, BackupStats, DailyPlaytime, SessionSummary, OBJECT_PATH, SERVICE_NAME,
};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tokio::sync::{mpsc, watch, Notify};
use tracing::{debug, error, info, instrument, warn};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::Connection;

use crate::backup::MIN_INTERVAL_SECONDS;
use crate::config::SharedConfig;
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
    config: SharedConfig,
    /// Notifies the backup scheduler when a backup-related setting
    /// has been mutated through this interface. The scheduler
    /// re-reads `config` and resets its timer so the change applies
    /// before the next snapshot, not after the in-flight one.
    backup_changed: Arc<Notify>,
}

impl Tracker {
    /// Construct a tracker bound to the given database handle, the
    /// shared blocklist the session manager also watches, and the
    /// shared tunable config the gate + foreground source read. All
    /// three handles are mutated in place by the corresponding setter
    /// RPCs so changes take effect on the very next event.
    #[must_use]
    pub fn new(
        db: Arc<Database>,
        blocklist: SharedBlocklist,
        config: SharedConfig,
        backup_changed: Arc<Notify>,
    ) -> Self {
        Self {
            db,
            blocklist,
            config,
            backup_changed,
        }
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
    ///
    /// Adjacent same-application sessions whose end-to-start gap is
    /// shorter than [`DEFAULT_MERGE_GAP_SECONDS`] are folded into
    /// single rows before serving — alt-tabbing to a chat window for
    /// a few seconds shouldn't fragment one play into N "sessions"
    /// in the GUI. The fragment count rides along on each row so a
    /// future "show fragments" toggle can rebuild the raw list. `limit`
    /// is the cap on raw rows fetched from the DB; merging only
    /// shrinks, so callers may receive fewer rows than they asked
    /// for and that's the desired behaviour (less noise).
    async fn list_recent_sessions(&self, limit: u32) -> zbus::fdo::Result<Vec<SessionSummary>> {
        let limit = limit.clamp(1, 1000);
        let rows = self
            .db
            .sessions()
            .list_recent_with_app(limit)
            .await
            .map_err(|e| into_fdo(&e))?;
        let merged = merge_adjacent_recent(
            rows,
            std::time::Duration::from_secs(DEFAULT_MERGE_GAP_SECONDS),
        );
        Ok(merged
            .into_iter()
            .map(|(row, frags)| SessionSummary {
                id: row.id,
                application_id: row.application_id,
                product_name: row.product_name,
                started_at: format_datetime(row.started_at),
                ended_at: row.ended_at.map(format_datetime).unwrap_or_default(),
                full_runtime_seconds: row.full_runtime_seconds,
                interactive_runtime_seconds: row.interactive_runtime_seconds,
                exit_reason: row.exit_reason.map(|r| r.to_string()).unwrap_or_default(),
                fragment_ids: frags,
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

    /// Delete a set of closed session rows by primary key. Used by the
    /// GUI's per-session delete affordance on the game-detail view: the
    /// caller passes every fragment id of the displayed merged span
    /// (`SessionSummary.fragment_ids`), so the rows dropped match
    /// exactly what was shown — the daemon never re-derives the span
    /// (PERSIST-2). For an unmerged row this is a single-element list.
    /// Returns `true` when at least one row was removed, `false` when
    /// none matched (already gone — no-op).
    ///
    /// The owning application's denormalized stats
    /// (`stat_run_count`, `stat_total_full`, `stat_total_interactive`,
    /// `stat_longest_full`, `last_played_at`) are recomputed from
    /// the surviving sessions in the same transaction.
    ///
    /// Refuses (deleting nothing) if any id is an open session — the
    /// session manager owns in-flight rows and silently dropping one
    /// would lose actively tracked runtime. The error message tells
    /// the user to stop the game first.
    async fn delete_session(&self, ids: Vec<i64>) -> zbus::fdo::Result<bool> {
        let removed = self
            .db
            .sessions()
            .delete_sessions_and_recompute(&ids)
            .await
            .map_err(|e| into_fdo(&e))?;
        if removed {
            info!(ids = ?ids, "session span deleted via D-Bus");
        }
        Ok(removed)
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
        let merged = merge_adjacent_session(
            sessions,
            std::time::Duration::from_secs(DEFAULT_MERGE_GAP_SECONDS),
        );
        Ok(merged
            .into_iter()
            .map(|(s, frags)| session_summary_for(application_id, product_name.clone(), &s, frags))
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

    /// Update the GPU memory threshold setting. The DB row is written
    /// first, then the in-memory shared config is updated so the gate
    /// picks up the new value on the next activation. No daemon
    /// restart required.
    async fn set_gpu_memory_threshold_bytes(&self, bytes: u64) -> zbus::fdo::Result<()> {
        self.db
            .settings()
            .set_u64(GPU_MEMORY_THRESHOLD_BYTES, bytes)
            .await
            .map_err(|e| into_fdo(&e))?;
        self.config.write().await.gate.gpu_memory_threshold_bytes = bytes;
        info!(gpu_memory_threshold_bytes = bytes, "setting updated");
        Ok(())
    }

    /// Grace window (seconds) the foreground source waits after a
    /// tracked game loses focus before closing the session.
    async fn get_alt_tab_grace_seconds(&self) -> zbus::fdo::Result<u64> {
        self.db
            .settings()
            .get_u64(ALT_TAB_GRACE_SECONDS, DEFAULT_ALT_TAB_GRACE_SECONDS)
            .await
            .map_err(|e| into_fdo(&e))
    }

    /// Update the alt-tab grace window. DB first, then the shared
    /// config so the very next backgrounded tracked window uses the
    /// new value.
    async fn set_alt_tab_grace_seconds(&self, seconds: u64) -> zbus::fdo::Result<()> {
        self.db
            .settings()
            .set_u64(ALT_TAB_GRACE_SECONDS, seconds)
            .await
            .map_err(|e| into_fdo(&e))?;
        self.config.write().await.alt_tab_grace = std::time::Duration::from_secs(seconds);
        info!(alt_tab_grace_seconds = seconds, "setting updated");
        Ok(())
    }

    /// Whether losing focus pauses the session. When `false`,
    /// sessions only end on process exit (GTT-parity behaviour).
    async fn get_pause_when_backgrounded(&self) -> zbus::fdo::Result<bool> {
        self.db
            .settings()
            .get_bool(PAUSE_WHEN_BACKGROUNDED, DEFAULT_PAUSE_WHEN_BACKGROUNDED)
            .await
            .map_err(|e| into_fdo(&e))
    }

    /// Update whether focus-loss pauses the session. DB first,
    /// then the shared config so the next activation reads the
    /// new value.
    async fn set_pause_when_backgrounded(&self, pause: bool) -> zbus::fdo::Result<()> {
        self.db
            .settings()
            .set_bool(PAUSE_WHEN_BACKGROUNDED, pause)
            .await
            .map_err(|e| into_fdo(&e))?;
        self.config.write().await.pause_when_backgrounded = pause;
        info!(pause_when_backgrounded = pause, "setting updated");
        Ok(())
    }

    /// Per-idle-interval grace (seconds). The first `grace` seconds
    /// of every input-idle interval are credited to interactive
    /// runtime instead of subtracted as AFK; covers cutscenes,
    /// dialogue trees, and similar engagement-without-input events.
    async fn get_idle_grace_seconds(&self) -> zbus::fdo::Result<u64> {
        self.db
            .settings()
            .get_u64(IDLE_GRACE_SECONDS, DEFAULT_IDLE_GRACE_SECONDS)
            .await
            .map_err(|e| into_fdo(&e))
    }

    /// Update the cutscene-grace window. DB first, then the shared
    /// config so the next heartbeat / close-session reads the new
    /// value — already-billed heartbeats stay as they were, but the
    /// next snapshot uses the updated grace.
    async fn set_idle_grace_seconds(&self, seconds: u64) -> zbus::fdo::Result<()> {
        self.db
            .settings()
            .set_u64(IDLE_GRACE_SECONDS, seconds)
            .await
            .map_err(|e| into_fdo(&e))?;
        self.config.write().await.idle_grace = std::time::Duration::from_secs(seconds);
        info!(idle_grace_seconds = seconds, "setting updated");
        Ok(())
    }

    /// Cadence (hours) between automatic database snapshots. The
    /// scheduler clamps anything below the safety floor at read
    /// time, so a returned value can technically be lower than the
    /// effective interval — the GUI is expected to apply the same
    /// floor in its input control.
    async fn get_backup_interval_hours(&self) -> zbus::fdo::Result<u64> {
        self.db
            .settings()
            .get_u64(BACKUP_INTERVAL_HOURS, DEFAULT_BACKUP_INTERVAL_HOURS)
            .await
            .map_err(|e| into_fdo(&e))
    }

    /// Update the backup cadence. Clamped to the scheduler's safety
    /// floor before persisting so the stored value matches what the
    /// scheduler actually uses; otherwise a "you saved 1h" toast
    /// would mislead a user that typed `0`. The scheduler is then
    /// notified to reset its timer immediately rather than waiting
    /// for the in-flight tick.
    async fn set_backup_interval_hours(&self, hours: u64) -> zbus::fdo::Result<()> {
        let floor_hours = MIN_INTERVAL_SECONDS / 3_600;
        let hours = hours.max(floor_hours);
        self.db
            .settings()
            .set_u64(BACKUP_INTERVAL_HOURS, hours)
            .await
            .map_err(|e| into_fdo(&e))?;
        self.config.write().await.backup.interval =
            std::time::Duration::from_secs(hours.saturating_mul(3_600));
        self.backup_changed.notify_one();
        info!(backup_interval_hours = hours, "setting updated");
        Ok(())
    }

    /// Number of snapshots the scheduler retains after each prune.
    async fn get_backup_retention_count(&self) -> zbus::fdo::Result<u64> {
        self.db
            .settings()
            .get_u64(BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_RETENTION_COUNT)
            .await
            .map_err(|e| into_fdo(&e))
    }

    /// Update the retention count and immediately prune the backup
    /// directory to that count. The prune routine clamps zero to one
    /// internally; we apply the same clamp at the setter so the
    /// stored value matches what the GUI sees on its next refresh.
    ///
    /// Pruning at save-time matches the user's mental model: typing
    /// a smaller number means "I want this many *now*", not "after
    /// the next snapshot eventually". The CLI's
    /// `ludex backup prune --keep N` already behaves this way.
    /// `prune_backups` itself enforces the `>= 1` floor regardless
    /// of what we hand it, so this path can never drop the on-disk
    /// set to zero.
    async fn set_backup_retention_count(&self, count: u64) -> zbus::fdo::Result<()> {
        let count = count.max(1);
        self.db
            .settings()
            .set_u64(BACKUP_RETENTION_COUNT, count)
            .await
            .map_err(|e| into_fdo(&e))?;
        let retention_usize = usize::try_from(count).unwrap_or(usize::MAX);
        self.config.write().await.backup.retention = retention_usize;
        self.backup_changed.notify_one();
        // Prune is best-effort: a missing backup directory or a
        // permission glitch shouldn't fail the setting save.
        // `default_backup_dir` returning `None` only happens in the
        // "neither XDG_DATA_HOME nor HOME is set" edge case the
        // scheduler already disables itself for; logging once and
        // moving on keeps the GUI flow snappy.
        if let Some(dir) = default_backup_dir() {
            match prune_backups(&dir, retention_usize) {
                Ok(removed) if !removed.is_empty() => {
                    info!(
                        removed = removed.len(),
                        retention = retention_usize,
                        "pruned older snapshots after retention save"
                    );
                }
                Ok(_) => {}
                Err(e) => warn!(error = %e, "prune-on-save failed"),
            }
        }
        info!(backup_retention_count = count, "setting updated");
        Ok(())
    }

    /// Take a snapshot now and prune to the configured retention,
    /// returning the absolute path of the new file. Routes through
    /// the same primitive the scheduler uses, so manual and
    /// automatic snapshots stay byte-compatible.
    async fn take_backup_now(&self) -> zbus::fdo::Result<String> {
        let path = snapshot_now(&self.db, None)
            .await
            .map_err(|e| into_fdo(&e))?;
        info!(path = %path.display(), "manual snapshot taken via D-Bus");
        Ok(path.display().to_string())
    }

    /// Summary of the on-disk backup set. The directory is always
    /// reported even when no backups exist, so the GUI can offer an
    /// "open folder" affordance without first taking a snapshot.
    #[allow(
        clippy::unused_async,
        reason = "kept async for symmetry with the rest of the interface; the directory is small and reading it doesn't need offloading"
    )]
    async fn get_backup_stats(&self) -> zbus::fdo::Result<BackupStats> {
        let dir = default_backup_dir().ok_or_else(|| {
            zbus::fdo::Error::Failed(
                "neither XDG_DATA_HOME nor HOME is set; cannot resolve backup dir".to_owned(),
            )
        })?;
        let entries = list_backups(&dir).map_err(|e| into_fdo(&e))?;
        let total_bytes: u64 = entries.iter().map(|e| e.size_bytes).sum();
        // Entries are sorted newest-first by `list_backups`; the
        // first one with a parseable timestamp wins. A directory
        // full of unparseable filenames reports an empty `latest_at`
        // rather than guessing from mtime — the GUI shows "—" then.
        let latest_at = entries
            .iter()
            .find_map(|e| e.timestamp)
            .and_then(|t| t.format(&Rfc3339).ok())
            .unwrap_or_default();
        Ok(BackupStats {
            directory: dir.display().to_string(),
            count: entries.len() as u64,
            total_bytes,
            latest_at,
        })
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

fn session_summary_for(
    application_id: i64,
    product_name: String,
    s: &Session,
    fragment_ids: Vec<i64>,
) -> SessionSummary {
    SessionSummary {
        id: s.id,
        application_id,
        product_name,
        started_at: format_datetime(s.started_at),
        ended_at: s.ended_at.map(format_datetime).unwrap_or_default(),
        full_runtime_seconds: s.full_runtime_seconds,
        interactive_runtime_seconds: s.interactive_runtime_seconds,
        exit_reason: s.exit_reason.map(|r| r.to_string()).unwrap_or_default(),
        fragment_ids,
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
pub async fn serve(
    db: Arc<Database>,
    blocklist: SharedBlocklist,
    config: SharedConfig,
    backup_changed: Arc<Notify>,
) -> anyhow::Result<Connection> {
    let tracker = Tracker::new(db, blocklist, config, backup_changed);
    // Build the connection without requesting the name in the
    // builder so we can supply explicit flags below. The default
    // path takes the name-queue slot if the name is already
    // owned — we want strict failure instead, so two daemons
    // can't silently share writes to the same SQLite file.
    let conn = zbus::connection::Builder::session()?
        .serve_at(OBJECT_PATH, tracker)?
        .build()
        .await?;
    let reply = conn
        .request_name_with_flags(SERVICE_NAME, RequestNameFlags::DoNotQueue.into())
        .await
        .context("request net.ludex.Tracker1 bus name")?;
    match reply {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
        RequestNameReply::Exists | RequestNameReply::InQueue => {
            anyhow::bail!(
                "another ludex-daemon already owns {SERVICE_NAME}; \
                 refusing to start a second instance. \
                 Run `pgrep -a ludex-daemon` to find the existing process."
            );
        }
    }
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
