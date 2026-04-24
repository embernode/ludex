//! Consumes [`GameEvent`]s and persists sessions.
//!
//! Invariants maintained by this layer:
//!
//! - At most one open session per [`GameKey`] at a time. A `Started` for an
//!   already-open key is a source-side bug; the manager logs and drops it.
//! - Every open session receives a heartbeat every
//!   [`HEARTBEAT_INTERVAL_SECS`] seconds. A crash after a heartbeat loses
//!   at most that interval's worth of runtime.
//! - Sessions open at daemon startup older than the grace period are
//!   closed at their last-known heartbeat with
//!   [`ExitReason::Recovered`](ludex_core::ExitReason::Recovered).
//! - On graceful shutdown (signal received) all open sessions are closed
//!   at `now` with [`ExitReason::Terminated`](ludex_core::ExitReason::Terminated).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ludex_core::{
    Application, Database, Error, ExitReason, GameKey, GraphicsPlatform, Icons, NewApplication,
    ProcessArchitecture, RuntimeSnapshot,
};
use ludex_enrich::EnrichmentContext;
use time::{Duration, OffsetDateTime};
use tokio::sync::{mpsc, watch, RwLock};
use tracing::{debug, error, info, instrument, warn};

/// Shared in-memory mirror of the `blocked_applications` table.
///
/// The session manager reads it on every `Started`; future GUI-side
/// writers (M6.6.3+) hold a clone of the same `Arc` so a D-Bus
/// `Block(appid)` call both updates the DB and flips the in-memory
/// set in one shot — no reload signal, no polling, no TOCTOU.
pub type SharedBlocklist = Arc<RwLock<HashSet<GameKey>>>;

use crate::dbus::TrackerNotification;
use crate::event::GameEvent;
use crate::idle::IdleTracker;
use crate::sleep::SleepTracker;

/// Heartbeat cadence in seconds. Also the upper bound on runtime lost to
/// a daemon crash.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 60;

/// Sessions whose last heartbeat is older than this at daemon startup are
/// considered orphaned and closed with `ExitReason::Recovered`.
pub const ORPHAN_GRACE_MINUTES: i64 = 2;

/// In-memory state for a session currently open in the database.
#[derive(Debug, Clone, Copy)]
struct OpenSession {
    session_id: i64,
    application_id: i64,
    started_at: OffsetDateTime,
    /// Cumulative idle seconds at session start. The delta against
    /// the tracker's current value is subtracted from the session's
    /// full runtime to yield the interactive runtime.
    baseline_idle_seconds: i64,
    /// Cumulative suspended seconds at session start. The delta is
    /// subtracted from wall-clock elapsed time before anything else;
    /// a system that suspended for eight hours mid-session must not
    /// count those eight hours as either full or interactive runtime.
    baseline_suspended_seconds: i64,
}

/// Stateful session bookkeeper.
pub struct SessionManager {
    db: Database,
    enrichment_ctx: Arc<EnrichmentContext>,
    idle_tracker: Arc<IdleTracker>,
    sleep_tracker: Arc<SleepTracker>,
    /// Optional channel to the D-Bus notifier task. `None` when no
    /// Tracker service is exposed (e.g. integration tests that don't
    /// stand up the public API).
    notifications: Option<mpsc::Sender<TrackerNotification>>,
    blocklist: SharedBlocklist,
    open: HashMap<GameKey, OpenSession>,
}

impl SessionManager {
    /// Construct a manager that reads/writes the given [`Database`],
    /// spawns enrichment with the given [`EnrichmentContext`] on new
    /// applications, and queries the shared [`IdleTracker`] and
    /// [`SleepTracker`] for the user's idle and system-suspended time
    /// during each session.
    ///
    /// `notifications` is the optional channel to the
    /// [`net.ludex.Tracker1`](crate::dbus) D-Bus notifier; pass
    /// `None` when no public API is exposed. `blocklist` is the
    /// shared in-memory mirror of the `blocked_applications` table;
    /// pass `Arc::new(RwLock::new(HashSet::new()))` when the caller
    /// doesn't care about blocking (tests that only exercise the
    /// session-lifecycle paths).
    #[must_use]
    pub fn new(
        db: Database,
        enrichment_ctx: Arc<EnrichmentContext>,
        idle_tracker: Arc<IdleTracker>,
        sleep_tracker: Arc<SleepTracker>,
        notifications: Option<mpsc::Sender<TrackerNotification>>,
        blocklist: SharedBlocklist,
    ) -> Self {
        Self {
            db,
            enrichment_ctx,
            idle_tracker,
            sleep_tracker,
            notifications,
            blocklist,
            open: HashMap::new(),
        }
    }

    fn notify(&self, notification: TrackerNotification) {
        let Some(tx) = self.notifications.as_ref() else {
            return;
        };
        // `try_send` rather than `send`: we do not want to block a
        // heartbeat or session-close path on a slow client. A full
        // channel means the notifier task is lagging — worth a log
        // line. A closed channel means the notifier has already
        // exited (e.g. daemon shutdown in progress) — silent.
        if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(notification) {
            warn!("tracker notification channel full; dropping signal");
        }
    }

    /// Close any sessions left open by a prior daemon run whose heartbeat
    /// is older than [`ORPHAN_GRACE_MINUTES`]. Returns the number of rows
    /// closed. Call once before [`Self::run`].
    ///
    /// Each orphan is closed with its last-known heartbeat runtime and
    /// rolled into the owning application's aggregate stats in a single
    /// transaction — the recovery path makes the same atomicity promise
    /// as the normal close path, so a crash during recovery cannot drop
    /// runtime from the application counters.
    pub async fn recover_orphans(&self) -> Result<u64, Error> {
        let cutoff = OffsetDateTime::now_utc() - Duration::minutes(ORPHAN_GRACE_MINUTES);
        let orphans = self.db.sessions().list_orphans(cutoff).await?;
        let count = orphans.len() as u64;
        for orphan in orphans {
            self.db
                .sessions()
                .close_and_rollup(
                    orphan.id,
                    orphan.application_id,
                    RuntimeSnapshot {
                        full_runtime_seconds: orphan.full_runtime_seconds,
                        interactive_runtime_seconds: orphan.interactive_runtime_seconds,
                        at: orphan.heartbeat_at,
                    },
                    ExitReason::Recovered,
                )
                .await?;
        }
        if count > 0 {
            info!(
                recovered = count,
                "closed orphaned sessions at last heartbeat"
            );
        }
        Ok(count)
    }

    /// Process events and heartbeat open sessions until `shutdown` fires
    /// or all event senders drop.
    pub async fn run(
        mut self,
        mut events: mpsc::Receiver<GameEvent>,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let mut heartbeat =
            tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
        // Discard the immediate-fire first tick so heartbeats begin after one
        // interval, not on startup.
        heartbeat.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("shutdown signalled");
                        break;
                    }
                }
                maybe_event = events.recv() => {
                    let Some(event) = maybe_event else {
                        info!("all event sources closed");
                        break;
                    };
                    if let Err(e) = self.handle_event(event).await {
                        error!(error = %e, "event handling failed");
                    }
                }
                _ = heartbeat.tick() => {
                    if let Err(e) = self.heartbeat_open_sessions().await {
                        error!(error = %e, "heartbeat failed");
                    }
                }
            }
        }

        // Close all still-open sessions so the DB is consistent when the
        // daemon exits cleanly.
        self.close_all_open(ExitReason::Terminated).await;
        Ok(())
    }

    #[instrument(skip(self, event), fields(event = ?event))]
    async fn handle_event(&mut self, event: GameEvent) -> Result<(), Error> {
        match event {
            GameEvent::Started {
                key,
                display_name,
                at,
            } => self.handle_started(key, display_name, at).await,
            GameEvent::Stopped { key, at } => self.handle_stopped(key, at).await,
        }
    }

    async fn handle_started(
        &mut self,
        key: GameKey,
        display_name: String,
        at: OffsetDateTime,
    ) -> Result<(), Error> {
        if self.blocklist.read().await.contains(&key) {
            info!(%key, "blocked application; not opening session");
            return Ok(());
        }
        if self.open.contains_key(&key) {
            warn!(%key, "Started received for already-open session; ignoring");
            return Ok(());
        }
        let app = self
            .find_or_create_application(&key, display_name, at)
            .await?;
        let session = self.db.sessions().begin(app.id, at).await?;
        let baseline_idle_seconds = self.idle_tracker.accumulated_idle_seconds();
        let baseline_suspended_seconds = self.sleep_tracker.accumulated_suspended_seconds();
        // Game title logged at debug only; info keeps the numeric
        // identifiers that are enough for correlation without
        // leaking play history into journalctl / stderr captures.
        debug!(
            app_id = app.id,
            session_id = session.id,
            product_name = %app.product_name,
            "session opened"
        );
        info!(
            app_id = app.id,
            session_id = session.id,
            baseline_idle_seconds,
            baseline_suspended_seconds,
            "session opened"
        );
        self.open.insert(
            key,
            OpenSession {
                session_id: session.id,
                application_id: app.id,
                started_at: at,
                baseline_idle_seconds,
                baseline_suspended_seconds,
            },
        );
        self.notify(TrackerNotification::SessionStarted {
            application_id: app.id,
        });
        Ok(())
    }

    async fn handle_stopped(&mut self, key: GameKey, at: OffsetDateTime) -> Result<(), Error> {
        if let Some(open) = self.open.remove(&key) {
            self.close_session(open, at, ExitReason::Terminated).await?;
        } else {
            warn!(%key, "Stopped received for unknown session; ignoring");
        }
        Ok(())
    }

    async fn find_or_create_application(
        &self,
        key: &GameKey,
        display_name: String,
        now: OffsetDateTime,
    ) -> Result<Application, Error> {
        if let Some(app) = self.db.applications().find_by_key(key).await? {
            return Ok(app);
        }
        let app = self
            .db
            .applications()
            .create(NewApplication {
                launcher_type: key.launcher_type,
                launcher_id: key.launcher_id.clone(),
                product_name: display_name,
                publisher: None,
                version: None,
                executable_path: None,
                launcher_exe_path: None,
                wineprefix_path: None,
                installed_flatpak_ref: None,
                graphics_platform: GraphicsPlatform::Unknown,
                process_architecture: ProcessArchitecture::Unknown,
                group_id: None,
                icons: Icons::default(),
                first_seen_at: now,
            })
            .await?;
        self.spawn_enrichment(app.id);
        self.notify(TrackerNotification::ApplicationAdded {
            application_id: app.id,
        });
        Ok(app)
    }

    /// Spawn the enrichment cascade against `app_id` on the runtime. The
    /// cascade reads every configured identity source (`.desktop`, PE
    /// `FileVersionInfo`, GOG `.info`, Steam `.acf`, etc.) and issues one
    /// `update_identity` call with whatever it finds. Errors are logged
    /// and swallowed; the handle is intentionally dropped — enrichment
    /// is best-effort and must never block or fail session tracking.
    fn spawn_enrichment(&self, app_id: i64) {
        let db = self.db.clone();
        let ctx = Arc::clone(&self.enrichment_ctx);
        tokio::spawn(async move {
            if let Err(e) = ludex_enrich::run_cascade(&db, &ctx, app_id).await {
                warn!(app_id, error = %e, "enrichment failed");
            }
        });
    }

    async fn heartbeat_open_sessions(&self) -> Result<(), Error> {
        if self.open.is_empty() {
            return Ok(());
        }
        let now = OffsetDateTime::now_utc();
        for open in self.open.values() {
            let (full, interactive) = self.runtimes_for(open, now);
            self.db
                .sessions()
                .heartbeat(
                    open.session_id,
                    RuntimeSnapshot {
                        full_runtime_seconds: full,
                        interactive_runtime_seconds: interactive,
                        at: now,
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// Compute full and interactive runtime seconds for an open
    /// session at `now`.
    ///
    /// Wall-clock elapsed time is the upper bound. From it we
    /// subtract system-suspend time to produce `full_runtime` — a
    /// session that survives an eight-hour laptop-closed stretch
    /// must not count those hours as gameplay. Interactive runtime
    /// further subtracts user-idle time (the user stepped away but
    /// the process kept running).
    ///
    /// Both outputs are clamped into `[0, full_runtime]` because the
    /// CHECK constraints on `sessions.*_runtime_seconds` reject
    /// negative values and require `interactive ≤ full`. A single
    /// buggy sample (spurious clock jump, NTP step) must not corrupt
    /// an in-flight heartbeat.
    fn runtimes_for(&self, open: &OpenSession, now: OffsetDateTime) -> (i64, i64) {
        let wall = (now - open.started_at).whole_seconds().max(0);
        let suspended_during = (self.sleep_tracker.accumulated_suspended_seconds()
            - open.baseline_suspended_seconds)
            .max(0);
        let full = (wall - suspended_during).clamp(0, wall);
        let idle_during =
            (self.idle_tracker.accumulated_idle_seconds() - open.baseline_idle_seconds).max(0);
        let interactive = (full - idle_during).clamp(0, full);
        (full, interactive)
    }

    async fn close_session(
        &self,
        open: OpenSession,
        ended_at: OffsetDateTime,
        reason: ExitReason,
    ) -> Result<(), Error> {
        let (full, interactive) = self.runtimes_for(&open, ended_at);
        self.db
            .sessions()
            .close_and_rollup(
                open.session_id,
                open.application_id,
                RuntimeSnapshot {
                    full_runtime_seconds: full,
                    interactive_runtime_seconds: interactive,
                    at: ended_at,
                },
                reason,
            )
            .await?;
        info!(
            app_id = open.application_id,
            session_id = open.session_id,
            full_seconds = full,
            reason = %reason.as_ref(),
            "session closed"
        );
        self.notify(TrackerNotification::SessionEnded {
            application_id: open.application_id,
            full_runtime_seconds: full,
            interactive_runtime_seconds: interactive,
        });
        Ok(())
    }

    async fn close_all_open(&mut self, reason: ExitReason) {
        let now = OffsetDateTime::now_utc();
        let to_close: Vec<OpenSession> = self.open.drain().map(|(_, v)| v).collect();
        for open in to_close {
            if let Err(e) = self.close_session(open, now, reason).await {
                error!(error = %e, "failed to close open session during shutdown");
            }
        }
    }
}
