//! Session-shaped queries.

use sqlx::SqlitePool;
use time::OffsetDateTime;

use crate::error::{Error, Result};
use crate::session::{DailyPlaytime, RecentSession, RuntimeSnapshot, Session};
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
    ///
    /// Returns [`Error::OpenSessionExists`] if an open session for
    /// this application already exists. That's normally impossible
    /// within a single daemon (the session manager dedupes by
    /// `GameKey` in memory) but can fire when two daemons are
    /// accidentally running against the same database; the partial
    /// unique index `one_open_session_per_app` catches the race.
    pub async fn begin(&self, application_id: i64, started_at: OffsetDateTime) -> Result<Session> {
        let sql = format!(
            "INSERT INTO sessions (application_id, started_at, heartbeat_at) \
             VALUES (?, ?, ?) RETURNING {SELECT_COLS}"
        );
        match sqlx::query_as::<_, Session>(&sql)
            .bind(application_id)
            .bind(started_at)
            .bind(started_at)
            .fetch_one(self.pool)
            .await
        {
            Ok(row) => Ok(row),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Err(Error::OpenSessionExists(application_id))
            }
            Err(e) => Err(e.into()),
        }
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

    /// Aggregate total runtime per calendar day for sessions that
    /// started at or after `cutoff`. Returns one row per day that has
    /// at least one session; days with no sessions are omitted
    /// (callers that need a continuous range fill zeros). Includes
    /// open sessions — their runtime is the most recent heartbeat
    /// value, which is good enough for a live dashboard.
    ///
    /// Sessions owned by applications present in the
    /// `blocked_applications` table are excluded — the dashboard
    /// should mirror the Games / Recent views, and hiding a game
    /// shouldn't still have it contributing to the heatmap totals.
    /// The subquery re-evaluates per call, so block/unblock changes
    /// surface on the very next fetch with no cache to invalidate.
    pub async fn daily_playtime_since(&self, cutoff: OffsetDateTime) -> Result<Vec<DailyPlaytime>> {
        // CAST around SUM/COUNT pins the result type to SQLite
        // INTEGER so sqlx's i64 decoder doesn't complain about the
        // default NUMERIC affinity of aggregate columns.
        let sql = "SELECT DATE(s.started_at) AS date, \
            CAST(COALESCE(SUM(s.full_runtime_seconds), 0) AS INTEGER) AS full_runtime_seconds, \
            CAST(COALESCE(SUM(s.interactive_runtime_seconds), 0) AS INTEGER) AS interactive_runtime_seconds, \
            CAST(COUNT(*) AS INTEGER) AS session_count \
            FROM sessions s \
            WHERE s.started_at >= ? \
              AND s.application_id NOT IN ( \
                SELECT a.id FROM applications a \
                JOIN blocked_applications b \
                  ON b.launcher_type = a.launcher_type \
                 AND b.launcher_id   = a.launcher_id \
              ) \
            GROUP BY DATE(s.started_at) \
            ORDER BY DATE(s.started_at) ASC";
        sqlx::query_as::<_, DailyPlaytime>(sql)
            .bind(cutoff)
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

    /// Delete the merged span that contains `session_id` and rebuild
    /// the owning application's denormalized aggregate stats from
    /// the rows that remain. The whole operation runs in one
    /// transaction so the application's counters can never observe
    /// a half-deleted span.
    ///
    /// "Merged span" means the same fold the GUI applies before
    /// rendering: consecutive same-application sessions whose
    /// end-to-start gap is `<= DEFAULT_MERGE_GAP_SECONDS` collapse
    /// into a single visible row. The user clicks delete on what
    /// they see, so the row set we drop has to match that fold —
    /// otherwise a "1 of 3 merged" delete would silently leave two
    /// orphan fragments behind. For an unmerged single-row span
    /// this collapses to deleting just that row, identical to the
    /// pre-merge behaviour.
    ///
    /// Returns `true` when at least one row was deleted, `false`
    /// when the id didn't match any row (already gone — no-op).
    /// Returns [`Error::Invariant`] when the requested session is
    /// open, or when the merged span containing it includes an
    /// open session as another fragment: the session manager is
    /// the authoritative writer for an in-flight session, and
    /// silently dropping its row mid-play would lose runtime that's
    /// actively being tracked. The user can stop the game and try
    /// again.
    ///
    /// Stats are rebuilt by re-querying the surviving sessions
    /// rather than computed-and-subtracted; that keeps
    /// `stat_longest_full` correct even when a deleted row was the
    /// previous longest, and avoids any drift between the counters
    /// and the row sums after enough deletes.
    pub async fn delete_and_recompute(&self, session_id: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await?;

        let row: Option<(i64, Option<OffsetDateTime>)> =
            sqlx::query_as("SELECT application_id, ended_at FROM sessions WHERE id = ?")
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((application_id, ended_at)) = row else {
            tx.rollback().await?;
            return Ok(false);
        };
        if ended_at.is_none() {
            tx.rollback().await?;
            return Err(Error::Invariant(
                "cannot delete an open session; stop the game first",
            ));
        }

        // Pull every session for this app, run the same merge fold
        // the GUI uses, find the span containing `session_id`, and
        // collect every fragment id in that span.
        let app_sessions: Vec<Session> = sqlx::query_as::<_, Session>(&format!(
            "SELECT {SELECT_COLS} FROM sessions \
             WHERE application_id = ? ORDER BY started_at DESC"
        ))
        .bind(application_id)
        .fetch_all(&mut *tx)
        .await?;

        // Snapshot which ids are currently open before the fold
        // consumes the vector — once merged, the per-fragment
        // `ended_at` is folded into the accumulator and we lose
        // direct visibility.
        let open_ids: std::collections::HashSet<i64> = app_sessions
            .iter()
            .filter(|s| s.ended_at.is_none())
            .map(|s| s.id)
            .collect();

        let merged = crate::session_merge::merge_adjacent_session(
            app_sessions,
            std::time::Duration::from_secs(crate::session_merge::DEFAULT_MERGE_GAP_SECONDS),
        );
        let Some((_, frags)) = merged
            .into_iter()
            .find(|(_, frags)| frags.contains(&session_id))
        else {
            // The lookup above proved `session_id` exists, the fold
            // ran on the same transaction, so the id must surface
            // in some span. Reaching this branch is a bug.
            tx.rollback().await?;
            return Err(Error::Invariant(
                "session id missing from merge fold output; bug",
            ));
        };

        if frags.iter().any(|fid| open_ids.contains(fid)) {
            tx.rollback().await?;
            return Err(Error::Invariant(
                "cannot delete a merged span that contains an open session; stop the game first",
            ));
        }

        // Bind ids one at a time inside the transaction. Typical
        // span has 1-5 fragments; building a dynamic `IN` clause
        // buys nothing here.
        for fid in &frags {
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(fid)
                .execute(&mut *tx)
                .await?;
        }

        sqlx::query(
            "UPDATE applications SET \
                stat_run_count         = (SELECT COUNT(*) FROM sessions WHERE application_id = ?1), \
                stat_total_full        = COALESCE((SELECT SUM(full_runtime_seconds) \
                                                     FROM sessions WHERE application_id = ?1), 0), \
                stat_total_interactive = COALESCE((SELECT SUM(interactive_runtime_seconds) \
                                                     FROM sessions WHERE application_id = ?1), 0), \
                stat_longest_full      = COALESCE((SELECT MAX(full_runtime_seconds) \
                                                     FROM sessions WHERE application_id = ?1), 0), \
                last_played_at         = (SELECT MAX(ended_at) FROM sessions \
                                            WHERE application_id = ?1 AND ended_at IS NOT NULL) \
             WHERE id = ?1",
        )
        .bind(application_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(true)
    }
}
