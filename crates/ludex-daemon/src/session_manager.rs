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

use std::collections::HashMap;

use ludex_core::{
    Application, Database, Error, ExitReason, GameKey, GraphicsPlatform, Icons, NewApplication,
    PlaybackDelta, ProcessArchitecture, RuntimeSnapshot,
};
use time::{Duration, OffsetDateTime};
use tokio::sync::{mpsc, watch};
use tracing::{error, info, instrument, warn};

use crate::event::GameEvent;

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
}

/// Stateful session bookkeeper.
pub struct SessionManager {
    db: Database,
    open: HashMap<GameKey, OpenSession>,
}

impl SessionManager {
    /// Construct a manager that reads/writes the given [`Database`].
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            db,
            open: HashMap::new(),
        }
    }

    /// Close any sessions left open by a prior daemon run whose heartbeat
    /// is older than [`ORPHAN_GRACE_MINUTES`]. Returns the number of rows
    /// closed. Call once before [`Self::run`].
    pub async fn recover_orphans(&self) -> Result<u64, Error> {
        let count = self
            .db
            .sessions()
            .recover_orphans(
                OffsetDateTime::now_utc(),
                Duration::minutes(ORPHAN_GRACE_MINUTES),
            )
            .await?;
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
        if self.open.contains_key(&key) {
            warn!(%key, "Started received for already-open session; ignoring");
            return Ok(());
        }
        let app = self
            .find_or_create_application(&key, display_name, at)
            .await?;
        let session = self.db.sessions().begin(app.id, at).await?;
        info!(
            app_id = app.id,
            session_id = session.id,
            product_name = %app.product_name,
            "session opened"
        );
        self.open.insert(
            key,
            OpenSession {
                session_id: session.id,
                application_id: app.id,
                started_at: at,
            },
        );
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
        self.db
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
            .await
    }

    async fn heartbeat_open_sessions(&self) -> Result<(), Error> {
        if self.open.is_empty() {
            return Ok(());
        }
        let now = OffsetDateTime::now_utc();
        for open in self.open.values() {
            let full = (now - open.started_at).whole_seconds().max(0);
            self.db
                .sessions()
                .heartbeat(
                    open.session_id,
                    RuntimeSnapshot {
                        full_runtime_seconds: full,
                        // Interactive accounting lands in M5; for now it
                        // mirrors full runtime.
                        interactive_runtime_seconds: full,
                        at: now,
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn close_session(
        &self,
        open: OpenSession,
        ended_at: OffsetDateTime,
        reason: ExitReason,
    ) -> Result<(), Error> {
        let full = (ended_at - open.started_at).whole_seconds().max(0);
        let interactive = full;
        self.db
            .sessions()
            .end(
                open.session_id,
                RuntimeSnapshot {
                    full_runtime_seconds: full,
                    interactive_runtime_seconds: interactive,
                    at: ended_at,
                },
                reason,
            )
            .await?;
        self.db
            .applications()
            .apply_playback(
                open.application_id,
                PlaybackDelta {
                    full_runtime_seconds: full,
                    interactive_runtime_seconds: interactive,
                    longest_full_candidate: Some(full),
                    last_played_at: ended_at,
                },
            )
            .await?;
        info!(
            app_id = open.application_id,
            session_id = open.session_id,
            full_seconds = full,
            reason = %reason.as_ref(),
            "session closed"
        );
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
