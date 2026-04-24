-- Enforce at the DB layer that no application has more than one
-- currently-open session. Partial unique index: only indexes rows
-- where `ended_at IS NULL`, so closed sessions don't participate
-- and the index stays cheap.
--
-- Motivation: if two ludex-daemon instances ever run at once
-- against the same database — e.g., the systemd --user service
-- and a fresh dev build of the binary, both writing to
-- `$XDG_DATA_HOME/ludex/ludex.sqlite` — each has its own in-memory
-- "open sessions" map and neither knows about the other. Both see
-- the same KWin activation, both run their session managers, and
-- both INSERT a row. The daemon owns bus name strictness to prevent
-- this going forward (see `dbus::serve`); this index is the
-- belt-and-suspenders layer so the DB itself refuses to hold the
-- impossible state.
--
-- Pre-existing orphan duplicates from previous multi-daemon runs
-- are collapsed first — keep the row with the highest id per
-- application (latest insert), close the rest with
-- `exit_reason = 'recovered'` and the best timestamp we can
-- synthesize. Using id-ordering means ties on `started_at` resolve
-- cleanly.

UPDATE sessions
SET
    ended_at = COALESCE(heartbeat_at, started_at),
    exit_reason = 'recovered'
WHERE
    ended_at IS NULL
    AND id NOT IN (
        SELECT MAX(id)
        FROM sessions
        WHERE ended_at IS NULL
        GROUP BY application_id
    );

-- Replace the existing non-unique partial index
-- (`idx_sessions_open` from migration 0001) with a unique one. The
-- shape is identical (same columns + predicate), so cold-start
-- recovery queries that scan `ended_at IS NULL` still hit an
-- index. Reusing the name keeps the schema tidy.
DROP INDEX IF EXISTS idx_sessions_open;
CREATE UNIQUE INDEX idx_sessions_open
    ON sessions(application_id)
    WHERE ended_at IS NULL;
