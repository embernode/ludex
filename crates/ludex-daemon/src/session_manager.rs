//! Consumes [`GameEvent`]s and persists sessions.
//!
//! Invariants maintained by this layer:
//!
//! - At most one open session per [`GameKey`] at a time. A `Started` for an
//!   already-open key is a source-side bug; the manager logs and drops it.
//! - Every open session receives a heartbeat every
//!   [`HEARTBEAT_INTERVAL_SECS`] seconds. A crash after a heartbeat loses
//!   at most that interval's worth of runtime.
//! - Any session still open at daemon startup is an orphan from a dead
//!   prior run (the single-instance lock rules out a live writer) and is
//!   closed at its last-known heartbeat with
//!   [`ExitReason::Recovered`](ludex_core::ExitReason::Recovered).
//! - On graceful shutdown (signal received) all open sessions are closed
//!   at `now` with [`ExitReason::Terminated`](ludex_core::ExitReason::Terminated).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ludex_core::{
    Application, Database, Error, ExitReason, GameKey, GraphicsPlatform, Icons, NewApplication,
    ProcessArchitecture, RuntimeSnapshot,
};
use ludex_enrich::EnrichmentContext;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch, RwLock};
use tracing::{debug, error, info, instrument, warn};

/// Shared in-memory mirror of the `blocked_applications` table.
///
/// The session manager reads it on every `Started`; future GUI-side
/// writers (M6.6.3+) hold a clone of the same `Arc` so a D-Bus
/// `Block(appid)` call both updates the DB and flips the in-memory
/// set in one shot — no reload signal, no polling, no TOCTOU.
pub type SharedBlocklist = Arc<RwLock<HashSet<GameKey>>>;

use crate::config::SharedConfig;
use crate::dbus::TrackerNotification;
use crate::event::GameEvent;
use crate::idle::IdleTracker;

/// Heartbeat cadence in seconds. Also the upper bound on runtime lost to
/// a daemon crash.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 60;

/// Monotonic clock used to measure session runtime.
///
/// A session's `full_runtime_seconds` is the elapsed [`Instant`] delta
/// between its start and the current sample — never a wall-clock
/// (`CLOCK_REALTIME`) difference. `CLOCK_MONOTONIC` (what [`Instant`]
/// reads on Linux) has the two properties runtime accounting needs:
/// it is immune to wall-clock jumps (an NTP step mid-session cannot
/// inflate or shrink recorded playtime) and it pauses while the system
/// is suspended (a laptop-closed stretch simply doesn't accrue, with
/// no separate suspend-subtraction step to get wrong). Production uses
/// [`SystemClock`]; tests inject a controllable clock.
pub trait Clock: Send + Sync {
    /// The current monotonic instant.
    fn now(&self) -> Instant;
}

/// Real monotonic clock backed by [`Instant::now`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// In-memory state for a session currently open in the database.
#[derive(Debug, Clone, Copy)]
struct OpenSession {
    session_id: i64,
    application_id: i64,
    /// Number of completed idle intervals on the [`IdleTracker`] at
    /// session start. The session's billable-idle calculation only
    /// considers intervals recorded after this baseline so that AFK
    /// time before the user picked up the controller doesn't get
    /// rolled into this play's stats.
    baseline_idle_intervals_count: usize,
    /// Seconds already elapsed on the idle interval that was open at
    /// session start, if any. Subtracted from that interval's billable
    /// idle so pre-session AFK time (and idle shared with an adjacent
    /// session) is never charged to this session.
    baseline_open_idle_seconds: i64,
    /// Monotonic instant captured when the session opened. Full runtime
    /// is measured as the delta from here to the current sample, so it
    /// is immune to wall-clock steps and excludes any time the system
    /// spent suspended (the monotonic clock pauses while asleep).
    started_instant: Instant,
}

/// Stateful session bookkeeper.
pub struct SessionManager {
    db: Database,
    enrichment_ctx: Arc<EnrichmentContext>,
    idle_tracker: Arc<IdleTracker>,
    clock: Arc<dyn Clock>,
    /// Shared tunable config. Read on each heartbeat / close so a
    /// `SetIdleGraceSeconds` D-Bus call lands on the very next
    /// runtime calculation — no daemon restart required.
    config: SharedConfig,
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
    /// applications, and queries the shared [`IdleTracker`] for the
    /// user's idle time during each session. `clock` is the monotonic
    /// clock each session's runtime is measured against — production
    /// passes [`SystemClock`].
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
        clock: Arc<dyn Clock>,
        config: SharedConfig,
        notifications: Option<mpsc::Sender<TrackerNotification>>,
        blocklist: SharedBlocklist,
    ) -> Self {
        Self {
            db,
            enrichment_ctx,
            idle_tracker,
            clock,
            config,
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

    /// Close every session left open by a prior daemon run. Returns the
    /// number of rows closed. Call once before [`Self::run`].
    ///
    /// Heartbeat age is deliberately not consulted: reaching this point
    /// means [`crate::dbus::serve`] already acquired the single-instance
    /// bus name, so no other daemon is writing and every open row is an
    /// orphan from a dead process. Filtering by age would strand a
    /// session whose owner crashed seconds ago — the common case, since
    /// systemd restarts the daemon within its `RestartSec` (5s), well
    /// inside any grace window. That orphan would then block the app's
    /// partial-unique open-session index and silently drop all further
    /// playtime until the next manual restart.
    ///
    /// Each orphan is closed with its last-known heartbeat runtime and
    /// rolled into the owning application's aggregate stats in a single
    /// transaction — the recovery path makes the same atomicity promise
    /// as the normal close path, so a crash during recovery cannot drop
    /// runtime from the application counters.
    pub async fn recover_orphans(&self) -> Result<u64, Error> {
        let orphans = self.db.sessions().list_all_orphans().await?;
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
                executable_path,
                at,
            } => {
                self.handle_started(key, display_name, executable_path, at)
                    .await
            }
            GameEvent::Stopped { key, at } => self.handle_stopped(key, at).await,
        }
    }

    async fn handle_started(
        &mut self,
        key: GameKey,
        display_name: String,
        executable_path: Option<PathBuf>,
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
            .find_or_create_application(&key, display_name, executable_path, at)
            .await?;
        let session = match self.db.sessions().begin(app.id, at).await {
            Ok(s) => s,
            // The DB-level uniqueness guard — see migration 0003 —
            // fires when some other writer already has an open
            // session for this application. In a correctly-running
            // single-daemon setup this is unreachable (the bus-
            // name lock in dbus::serve prevents two daemons from
            // ever both running); if we hit it anyway, some other
            // process owns the row and will close it eventually.
            // We drop our side silently rather than forcing a
            // close that would strand their in-memory state.
            Err(Error::OpenSessionExists(app_id)) => {
                warn!(
                    %key,
                    app_id,
                    "open session already exists in the database; \
                     dropping Start (another ludex-daemon may be running — check `pgrep -a ludex-daemon`)"
                );
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let (baseline_idle_intervals_count, baseline_open_idle_seconds) =
            self.idle_tracker.session_start_baseline();
        let started_instant = self.clock.now();
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
            baseline_idle_intervals_count,
            "session opened"
        );
        self.open.insert(
            key,
            OpenSession {
                session_id: session.id,
                application_id: app.id,
                baseline_idle_intervals_count,
                baseline_open_idle_seconds,
                started_instant,
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
        executable_path: Option<PathBuf>,
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
                executable_path: executable_path.map(|p| p.to_string_lossy().into_owned()),
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
        let now_instant = self.clock.now();
        // Read the live grace setting once per heartbeat batch — the
        // alternative (one read per open session) would be redundant
        // because in practice there's at most one open session.
        let grace_seconds =
            i64::try_from(self.config.read().await.idle_grace.as_secs()).unwrap_or(i64::MAX);
        for open in self.open.values() {
            let (full, interactive) = self.runtimes_for(open, now_instant, grace_seconds);
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
    /// session sampled at monotonic instant `now_instant`, applying
    /// `grace_seconds` of cutscene forgiveness to each natural idle
    /// interval.
    ///
    /// `full_runtime` is the monotonic elapsed time since the session
    /// opened. Reading it from `CLOCK_MONOTONIC` rather than the wall
    /// clock means an NTP step mid-session can neither inflate nor
    /// shrink it, and a laptop-closed suspend contributes nothing
    /// because the monotonic clock pauses while asleep — no separate
    /// suspend subtraction is required. Interactive runtime further
    /// subtracts billable idle time: each natural input-idle interval
    /// is forgiven up to `grace_seconds` (the typical cutscene length)
    /// and only the tail beyond that counts as the user genuinely
    /// stepping away.
    ///
    /// `interactive` is clamped into `[0, full_runtime]` because the
    /// CHECK constraints on `sessions.*_runtime_seconds` reject
    /// negative values and require `interactive ≤ full`.
    ///
    /// Both endpoints are sampled at *event-processing* time (the
    /// session's `started_instant` at `Started`, `now_instant` here),
    /// not from the event's wall-clock `at`. This is correct only while
    /// sources emit `at ≈ now`; every current source stamps `at =
    /// now_utc()` at emission and the manager drains its channel with
    /// sub-millisecond lag, so the two anchors cancel. A future source
    /// that backdates `Stopped.at` (e.g. a poll-detected exit time)
    /// would make `full` over-count the detection lag — such a source
    /// must carry its own monotonic anchor instead.
    fn runtimes_for(
        &self,
        open: &OpenSession,
        now_instant: Instant,
        grace_seconds: i64,
    ) -> (i64, i64) {
        let full = i64::try_from(
            now_instant
                .saturating_duration_since(open.started_instant)
                .as_secs(),
        )
        .unwrap_or(i64::MAX);
        let billable_idle = self.idle_tracker.billable_idle_seconds_since(
            open.baseline_idle_intervals_count,
            open.baseline_open_idle_seconds,
            grace_seconds,
        );
        let interactive = (full - billable_idle).clamp(0, full);
        (full, interactive)
    }

    async fn close_session(
        &self,
        open: OpenSession,
        ended_at: OffsetDateTime,
        reason: ExitReason,
    ) -> Result<(), Error> {
        let grace_seconds =
            i64::try_from(self.config.read().await.idle_grace.as_secs()).unwrap_or(i64::MAX);
        let (full, interactive) = self.runtimes_for(&open, self.clock.now(), grace_seconds);
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
