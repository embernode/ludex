//! Session-shaped queries.

use sqlx::SqlitePool;
use time::{OffsetDateTime, UtcOffset};

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

    /// Every session overlapping the half-open window `[from, to)`,
    /// oldest first, joined to the owning application's display
    /// identity.
    ///
    /// A session counts as overlapping when it started before `to` and
    /// had not already ended at `from` — so a play that crossed
    /// midnight belongs to both days it touched, and an open session
    /// (`ended_at IS NULL`) that began before the window is included
    /// because it is still running inside it.
    ///
    /// Bounded by the window rather than by a row count on purpose:
    /// the activity grid needs *all* of a day's sessions, and a
    /// newest-N fetch silently drops the older ones as soon as a busy
    /// week overflows the limit.
    pub async fn list_in_range(
        &self,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Result<Vec<RecentSession>> {
        // Timestamps are TEXT and sqlx encodes an `OffsetDateTime`
        // preserving its offset, so SQLite compares them bytewise.
        // A bound carrying `+02:00` would therefore be compared as a
        // string against stored UTC and silently select the wrong
        // rows; normalise both ends first.
        let from = from.to_offset(UtcOffset::UTC);
        let to = to.to_offset(UtcOffset::UTC);
        let sql = "SELECT \
                s.id, s.application_id, a.product_name, a.launcher_type, a.launcher_id, \
                s.started_at, s.ended_at, \
                s.full_runtime_seconds, s.interactive_runtime_seconds, \
                s.exit_reason \
             FROM sessions s \
             INNER JOIN applications a ON a.id = s.application_id \
             WHERE s.started_at < ? AND (s.ended_at IS NULL OR s.ended_at > ?) \
             ORDER BY s.started_at ASC";
        sqlx::query_as::<_, RecentSession>(sql)
            .bind(to)
            .bind(from)
            .fetch_all(self.pool)
            .await
            .map_err(Into::into)
    }

    /// Aggregate total runtime per *local* calendar day for sessions on
    /// or after `cutoff`'s local calendar day. Both the filter and the
    /// bucketing use SQLite's `localtime` modifier, which converts each
    /// stored UTC timestamp with the daemon's system timezone (DST
    /// handled per timestamp), so an evening session lands on the day the
    /// user actually played it rather than the next UTC day. The filter
    /// compares whole local days (`DATE(started_at,'localtime') >=
    /// DATE(cutoff,'localtime')`) rather than the raw instant, so the
    /// oldest bucket includes the *entire* cutoff day — a session logged
    /// before the cutoff's time-of-day is not silently dropped.
    /// Returns one row per day that has
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
        let sql = "SELECT DATE(s.started_at, 'localtime') AS date, \
            CAST(COALESCE(SUM(s.full_runtime_seconds), 0) AS INTEGER) AS full_runtime_seconds, \
            CAST(COALESCE(SUM(s.interactive_runtime_seconds), 0) AS INTEGER) AS interactive_runtime_seconds, \
            CAST(COUNT(*) AS INTEGER) AS session_count \
            FROM sessions s \
            WHERE DATE(s.started_at, 'localtime') >= DATE(?, 'localtime') \
              AND s.application_id NOT IN ( \
                SELECT a.id FROM applications a \
                JOIN blocked_applications b \
                  ON b.launcher_type = a.launcher_type \
                 AND b.launcher_id   = a.launcher_id \
              ) \
            GROUP BY DATE(s.started_at, 'localtime') \
            ORDER BY DATE(s.started_at, 'localtime') ASC";
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
    /// Every open (`ended_at IS NULL`) session, regardless of heartbeat
    /// age. At daemon cold start these are all orphans left by a dead
    /// prior process: the single-instance bus-name lock (acquired
    /// before recovery runs) guarantees no other daemon is writing, so
    /// a fresh process holds no live session and any open row must be
    /// recovered. Filtering by heartbeat age here would strand a
    /// session whose owner crashed seconds ago — exactly the common
    /// case, since systemd restarts the daemon within `RestartSec`.
    pub async fn list_all_orphans(&self) -> Result<Vec<Session>> {
        let sql = format!("SELECT {SELECT_COLS} FROM sessions WHERE ended_at IS NULL");
        sqlx::query_as::<_, Session>(&sql)
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

        let closed = sqlx::query(
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

        // The `ended_at IS NULL` guard matched nothing: the session is
        // already closed (or never existed). Its runtime is already in
        // the aggregates, so running the rollup again would double-count
        // — bail out and let the transaction roll back empty.
        if closed.rows_affected() == 0 {
            return Ok(());
        }

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

    /// Delete the given session rows and rebuild the owning
    /// application(s)' denormalized aggregate stats from the rows that
    /// remain. The whole operation runs in one transaction so the
    /// counters can never observe a half-deleted set.
    ///
    /// `ids` is the exact set of database rows to drop. For a merged
    /// span the GUI displayed, that is every fragment id the daemon
    /// folded into the visible span (carried on `SessionSummary`'s
    /// `fragment_ids` and passed straight back here) — so the rows we
    /// delete always match what the user saw, and a delete can never
    /// reach older fragments outside the displayed window (PERSIST-2).
    /// This method is deliberately span-agnostic: the fold happens
    /// once, at list time, and the caller owns the id set. For a
    /// single unmerged row `ids` is just `[id]`.
    ///
    /// Returns `true` when at least one row was deleted, `false` when
    /// none of the ids matched a row (already gone — no-op) or `ids`
    /// is empty. Returns [`Error::Invariant`] when any requested id is
    /// an open session: the session manager is the authoritative
    /// writer for an in-flight row, and silently dropping it mid-play
    /// would lose runtime that's actively being tracked. The user can
    /// stop the game and try again. Nothing is deleted in that case.
    ///
    /// Stats are rebuilt by re-querying the surviving sessions rather
    /// than computed-and-subtracted; that keeps `stat_longest_full`
    /// correct even when a deleted row was the previous longest, and
    /// avoids any drift between the counters and the row sums after
    /// enough deletes. The rebuild counts only *closed* rows: an open
    /// session's runtime is still being written and is added to these
    /// aggregates by `close_and_rollup` when it ends — counting it
    /// here too would double-count it (PERSIST-1).
    pub async fn delete_sessions_and_recompute(&self, ids: &[i64]) -> Result<bool> {
        if ids.is_empty() {
            return Ok(false);
        }

        let mut tx = self.pool.begin().await?;

        // Resolve each id to its owning application and open/closed
        // state. Missing ids (already deleted) are skipped; any open
        // id aborts the whole operation before a single row is dropped.
        let mut app_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut matched_any = false;
        for &id in ids {
            let row: Option<(i64, Option<OffsetDateTime>)> =
                sqlx::query_as("SELECT application_id, ended_at FROM sessions WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            let Some((application_id, ended_at)) = row else {
                continue;
            };
            if ended_at.is_none() {
                tx.rollback().await?;
                return Err(Error::Invariant(
                    "cannot delete an open session; stop the game first",
                ));
            }
            matched_any = true;
            app_ids.insert(application_id);
        }

        if !matched_any {
            tx.rollback().await?;
            return Ok(false);
        }

        // Delete the requested rows. Bind ids one at a time inside the
        // transaction; a typical span has 1-5 fragments, so a dynamic
        // `IN` clause buys nothing.
        for &id in ids {
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }

        // Rebuild each touched application's aggregates from its
        // surviving *closed* rows. Normally a single app, but a caller
        // that mixed apps in one call gets each reconciled.
        for application_id in app_ids {
            sqlx::query(
                "UPDATE applications SET \
                    stat_run_count         = (SELECT COUNT(*) FROM sessions \
                                                WHERE application_id = ?1 AND ended_at IS NOT NULL), \
                    stat_total_full        = COALESCE((SELECT SUM(full_runtime_seconds) \
                                                         FROM sessions \
                                                         WHERE application_id = ?1 AND ended_at IS NOT NULL), 0), \
                    stat_total_interactive = COALESCE((SELECT SUM(interactive_runtime_seconds) \
                                                         FROM sessions \
                                                         WHERE application_id = ?1 AND ended_at IS NOT NULL), 0), \
                    stat_longest_full      = COALESCE((SELECT MAX(full_runtime_seconds) \
                                                         FROM sessions \
                                                         WHERE application_id = ?1 AND ended_at IS NOT NULL), 0), \
                    last_played_at         = (SELECT MAX(ended_at) FROM sessions \
                                                WHERE application_id = ?1 AND ended_at IS NOT NULL) \
                 WHERE id = ?1",
            )
            .bind(application_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(true)
    }
}
