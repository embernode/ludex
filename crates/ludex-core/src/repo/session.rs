//! Session-shaped queries.

use sqlx::SqlitePool;
use time::OffsetDateTime;

use crate::error::Result;
use crate::session::{RecentSession, RuntimeSnapshot, Session};
use crate::types::ExitReason;

const SELECT_COLS: &str = "id, application_id, started_at, ended_at, heartbeat_at, \
    full_runtime_seconds, interactive_runtime_seconds, exit_reason";

/// Typed access to the `sessions` table.
pub struct SessionRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SessionRepo<'a> {
    /// Create a new repository bound to the given pool.
    #[must_use]
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Open a new session for the given application. The row is created
    /// with `ended_at = NULL` and the heartbeat equal to `started_at`.
    pub async fn begin(&self, application_id: i64, started_at: OffsetDateTime) -> Result<Session> {
        let sql = format!(
            "INSERT INTO sessions (application_id, started_at, heartbeat_at) \
             VALUES (?, ?, ?) RETURNING {SELECT_COLS}"
        );
        sqlx::query_as::<_, Session>(&sql)
            .bind(application_id)
            .bind(started_at)
            .bind(started_at)
            .fetch_one(self.pool)
            .await
            .map_err(Into::into)
    }

    /// Update the heartbeat and runtime counters for an open session.
    /// The session manager calls this roughly every 60 seconds. A crash
    /// after the most recent heartbeat loses at most that interval's
    /// worth of runtime.
    pub async fn heartbeat(&self, session_id: i64, snapshot: RuntimeSnapshot) -> Result<()> {
        sqlx::query(
            "UPDATE sessions \
             SET heartbeat_at = ?, full_runtime_seconds = ?, interactive_runtime_seconds = ? \
             WHERE id = ? AND ended_at IS NULL",
        )
        .bind(snapshot.at)
        .bind(snapshot.full_runtime_seconds)
        .bind(snapshot.interactive_runtime_seconds)
        .bind(session_id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Close an open session.
    pub async fn end(
        &self,
        session_id: i64,
        snapshot: RuntimeSnapshot,
        reason: ExitReason,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sessions \
             SET ended_at = ?, heartbeat_at = ?, \
                 full_runtime_seconds = ?, interactive_runtime_seconds = ?, \
                 exit_reason = ? \
             WHERE id = ? AND ended_at IS NULL",
        )
        .bind(snapshot.at)
        .bind(snapshot.at)
        .bind(snapshot.full_runtime_seconds)
        .bind(snapshot.interactive_runtime_seconds)
        .bind(reason)
        .bind(session_id)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Return a session row by primary key.
    pub async fn find_by_id(&self, id: i64) -> Result<Option<Session>> {
        let sql = format!("SELECT {SELECT_COLS} FROM sessions WHERE id = ?");
        sqlx::query_as::<_, Session>(&sql)
            .bind(id)
            .fetch_optional(self.pool)
            .await
            .map_err(Into::into)
    }

    /// List the `limit` most recent sessions for an application
    /// (closed or open).
    pub async fn list_for_application(
        &self,
        application_id: i64,
        limit: u32,
    ) -> Result<Vec<Session>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM sessions \
             WHERE application_id = ? ORDER BY started_at DESC LIMIT ?"
        );
        sqlx::query_as::<_, Session>(&sql)
            .bind(application_id)
            .bind(i64::from(limit))
            .fetch_all(self.pool)
            .await
            .map_err(Into::into)
    }

    /// List the `limit` most recent sessions across all applications,
    /// joined to the owning application's display identity.
    pub async fn list_recent_with_app(&self, limit: u32) -> Result<Vec<RecentSession>> {
        let sql = "SELECT \
                s.id, s.application_id, a.product_name, a.launcher_type, a.launcher_id, \
                s.started_at, s.ended_at, \
                s.full_runtime_seconds, s.interactive_runtime_seconds, \
                s.exit_reason \
             FROM sessions s \
             INNER JOIN applications a ON a.id = s.application_id \
             ORDER BY s.started_at DESC LIMIT ?";
        sqlx::query_as::<_, RecentSession>(sql)
            .bind(i64::from(limit))
            .fetch_all(self.pool)
            .await
            .map_err(Into::into)
    }

    /// Return every session with `ended_at IS NULL` whose last heartbeat
    /// is older than `cutoff`. Used by the daemon's cold-start recovery
    /// to locate sessions left open by a prior crashed run.
    ///
    /// The caller is expected to close each row with
    /// [`Self::close_and_rollup`] so the application-level aggregate
    /// stats are also updated.
    pub async fn list_orphans(&self, cutoff: OffsetDateTime) -> Result<Vec<Session>> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM sessions \
             WHERE ended_at IS NULL AND heartbeat_at < ?"
        );
        sqlx::query_as::<_, Session>(&sql)
            .bind(cutoff)
            .fetch_all(self.pool)
            .await
            .map_err(Into::into)
    }

    /// Close a session and roll its runtime into the owning application's
    /// aggregate statistics in a single transaction.
    ///
    /// Supersedes paired [`Self::end`] +
    /// [`ApplicationRepo::apply_playback`](crate::repo::ApplicationRepo::apply_playback)
    /// calls. Those are still exported for callers that need finer
    /// control, but every session-close path — normal shutdown,
    /// foreground change, `pidfd`-observed exit, cold-start orphan
    /// recovery — should prefer this so a crash between the two writes
    /// cannot leave the aggregate counters missing a session's runtime.
    pub async fn close_and_rollup(
        &self,
        session_id: i64,
        application_id: i64,
        snapshot: RuntimeSnapshot,
        reason: ExitReason,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "UPDATE sessions \
             SET ended_at = ?, heartbeat_at = ?, \
                 full_runtime_seconds = ?, interactive_runtime_seconds = ?, \
                 exit_reason = ? \
             WHERE id = ? AND ended_at IS NULL",
        )
        .bind(snapshot.at)
        .bind(snapshot.at)
        .bind(snapshot.full_runtime_seconds)
        .bind(snapshot.interactive_runtime_seconds)
        .bind(reason)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE applications \
             SET stat_run_count         = stat_run_count + 1, \
                 stat_total_full        = stat_total_full + ?, \
                 stat_total_interactive = stat_total_interactive + ?, \
                 stat_longest_full      = MAX(stat_longest_full, ?), \
                 last_played_at         = ? \
             WHERE id = ?",
        )
        .bind(snapshot.full_runtime_seconds)
        .bind(snapshot.interactive_runtime_seconds)
        .bind(snapshot.full_runtime_seconds)
        .bind(snapshot.at)
        .bind(application_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }
}
