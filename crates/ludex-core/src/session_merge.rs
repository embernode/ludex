//! Display-level fold for adjacent same-application sessions.
//!
//! A user who alt-tabs to a browser for a few seconds today produces
//! two database session rows — one closed by `foreground_changed`
//! and one fresh row when focus returns. The rows themselves are
//! correct; the noise is in *displaying* them as separate plays. This
//! module collapses runs of consecutive same-application rows whose
//! end-to-start gap is shorter than a caller-provided threshold into
//! single spans, leaving the database rows untouched.
//!
//! The fold is deliberately at the presentation layer:
//!
//! * It runs over the slice the GUI/CLI is about to render, so the
//!   underlying rows stay immutable and the daemon's
//!   `recover_orphans` path keeps its "open sessions are at most one
//!   per app" invariant intact.
//! * Toggling between merged and raw views is a parameter to this
//!   function — no migration, no schema change, no data lost.
//!
//! Callers feed in newest-first slices (the natural order for both
//! `list_recent_with_app` and `list_for_application`); the fold
//! returns newest-first merged spans paired with the database ids of
//! the fragments that fused into each span (in input order — newest
//! id first). The fragment-count field on the wire (`fragment_count`
//! on `SessionSummary`) is just `frags.len()`. The id list is what
//! lets `delete_and_recompute` drop a whole merged span as one
//! transaction without re-running the fold against the database.

use std::time::Duration;

use crate::session::{RecentSession, Session};

/// Default merge gap (seconds) the daemon applies before serving
/// session lists to the GUI. Tuned for "a quick alt-tab to a guide /
/// chat / browser" — long enough to absorb the typical fragmentation
/// pattern, short enough that two genuinely separate plays in the
/// same minute don't get fused.
pub const DEFAULT_MERGE_GAP_SECONDS: u64 = 60;

/// Fold consecutive same-application [`RecentSession`] rows whose
/// end-to-start gap is `<= gap` into single merged spans. Input must
/// be sorted newest-first; the returned vector preserves that order.
///
/// Returned tuples carry the merged span and the ids of the
/// underlying database rows that fused into it (newest id first,
/// matching input order). A non-merging row produces a single-element
/// id vector. The id vector is what lets the repo's
/// `delete_and_recompute` drop every fragment of a merged span in
/// one transaction without re-running the fold.
///
/// `gap == Duration::ZERO` is treated as "merge only when consecutive
/// fragments are touching" — practically a no-op, kept as a clean
/// disabled state for callers that want to bypass merging without a
/// separate code path.
#[must_use]
pub fn merge_adjacent_recent(
    rows: Vec<RecentSession>,
    gap: Duration,
) -> Vec<(RecentSession, Vec<i64>)> {
    fold(
        rows,
        gap,
        |row| row.application_id,
        |row| row.id,
        |row| row.started_at,
        |row| row.ended_at,
        merge_recent,
    )
}

/// [`merge_adjacent_recent`] for the bare [`Session`] shape (no
/// application identity attached). Used by per-application listings
/// where the application id is the route parameter, not part of the
/// row.
#[must_use]
pub fn merge_adjacent_session(rows: Vec<Session>, gap: Duration) -> Vec<(Session, Vec<i64>)> {
    fold(
        rows,
        gap,
        |row| row.application_id,
        |row| row.id,
        |row| row.started_at,
        |row| row.ended_at,
        merge_session,
    )
}

/// Generic fold parameterised on the row type. Kept private — the
/// closure-heavy signature is a means, not a public API. The two
/// public helpers above pin the closures so callers see a clean
/// interface.
fn fold<T, FApp, FId, FStart, FEnd, FMerge>(
    rows: Vec<T>,
    gap: Duration,
    application_id_of: FApp,
    id_of: FId,
    started_at_of: FStart,
    ended_at_of: FEnd,
    merge_into: FMerge,
) -> Vec<(T, Vec<i64>)>
where
    FApp: Fn(&T) -> i64,
    FId: Fn(&T) -> i64,
    FStart: Fn(&T) -> time::OffsetDateTime,
    FEnd: Fn(&T) -> Option<time::OffsetDateTime>,
    FMerge: Fn(&mut T, &T),
{
    // Convert gap to time::Duration once; `time` and `std::time` use
    // different types, and we compare with `time` arithmetic below.
    let gap = time::Duration::seconds_f64(gap.as_secs_f64());
    let mut out: Vec<(T, Vec<i64>)> = Vec::with_capacity(rows.len());
    for row in rows {
        let row_id = id_of(&row);
        let Some((acc, frags)) = out.last_mut() else {
            out.push((row, vec![row_id]));
            continue;
        };
        // Same application + the *older* row's `ended_at` is within
        // `gap` of the accumulator's `started_at`? If yes, fuse.
        // The accumulator is the newer span (head of the
        // newest-first output); `row` is the older candidate.
        let same_app = application_id_of(acc) == application_id_of(&row);
        let mergeable = same_app
            && match ended_at_of(&row) {
                // An older row that's still open is a contradiction
                // (DB partial-unique index allows one open per app),
                // but if it ever happens we refuse to merge — better
                // to surface the anomaly than hide it behind a fold.
                None => false,
                Some(end) => started_at_of(acc) - end <= gap,
            };
        if mergeable {
            merge_into(acc, &row);
            frags.push(row_id);
        } else {
            out.push((row, vec![row_id]));
        }
    }
    out
}

/// Mutate `acc` to absorb the older row `older`. Caller has already
/// verified same application id and gap eligibility.
fn merge_recent(acc: &mut RecentSession, older: &RecentSession) {
    // Extend the merged span backward in time.
    acc.started_at = older.started_at;
    acc.full_runtime_seconds = acc
        .full_runtime_seconds
        .saturating_add(older.full_runtime_seconds);
    acc.interactive_runtime_seconds = acc
        .interactive_runtime_seconds
        .saturating_add(older.interactive_runtime_seconds);
    // ended_at, exit_reason, id, product_name stay as the
    // accumulator's (newest fragment's) — the merged span "ends" the
    // way the latest fragment ended, and the newest id is a stable
    // React/Svelte key.
}

fn merge_session(acc: &mut Session, older: &Session) {
    acc.started_at = older.started_at;
    acc.full_runtime_seconds = acc
        .full_runtime_seconds
        .saturating_add(older.full_runtime_seconds);
    acc.interactive_runtime_seconds = acc
        .interactive_runtime_seconds
        .saturating_add(older.interactive_runtime_seconds);
    // heartbeat_at stays as the accumulator's: it represents
    // "freshest known activity" and the newest fragment owns that.
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn r(
        id: i64,
        app: i64,
        start: time::OffsetDateTime,
        end: time::OffsetDateTime,
    ) -> RecentSession {
        RecentSession {
            id,
            application_id: app,
            product_name: format!("App{app}"),
            launcher_type: crate::types::LauncherType::Native,
            launcher_id: format!("app-{app}"),
            started_at: start,
            ended_at: Some(end),
            full_runtime_seconds: (end - start).whole_seconds(),
            interactive_runtime_seconds: (end - start).whole_seconds(),
            exit_reason: Some(crate::types::ExitReason::ForegroundChanged),
        }
    }

    fn r_open(id: i64, app: i64, start: time::OffsetDateTime) -> RecentSession {
        RecentSession {
            id,
            application_id: app,
            product_name: format!("App{app}"),
            launcher_type: crate::types::LauncherType::Native,
            launcher_id: format!("app-{app}"),
            started_at: start,
            ended_at: None,
            full_runtime_seconds: 0,
            interactive_runtime_seconds: 0,
            exit_reason: None,
        }
    }

    #[test]
    fn empty_input_merges_to_empty() {
        let merged = merge_adjacent_recent(Vec::new(), Duration::from_mins(1));
        assert!(merged.is_empty());
    }

    #[test]
    fn unmergeable_rows_pass_through_with_count_one() {
        // Two different apps, no merging.
        let rows = vec![
            r(
                2,
                1,
                datetime!(2026-01-01 12:10 UTC),
                datetime!(2026-01-01 12:20 UTC),
            ),
            r(
                1,
                2,
                datetime!(2026-01-01 12:00 UTC),
                datetime!(2026-01-01 12:05 UTC),
            ),
        ];
        let merged = merge_adjacent_recent(rows, Duration::from_mins(1));
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|(_, frags)| frags.len() == 1));
    }

    #[test]
    fn small_gap_same_app_merges() {
        // Newest at top: ends 12:30. Older below: ends 12:14:30,
        // newer starts 12:15:00 — gap 30s, under threshold.
        let newer = r(
            2,
            1,
            datetime!(2026-01-01 12:15 UTC),
            datetime!(2026-01-01 12:30 UTC),
        );
        let older = r(
            1,
            1,
            datetime!(2026-01-01 12:00 UTC),
            datetime!(2026-01-01 12:14:30 UTC),
        );
        let merged = merge_adjacent_recent(vec![newer, older], Duration::from_mins(1));
        assert_eq!(merged.len(), 1);
        let (span, frags) = &merged[0];
        assert_eq!(frags, &vec![2, 1], "fragment ids in input order (newest first)");
        assert_eq!(span.id, 2, "id stays as the newest fragment's");
        assert_eq!(span.started_at, datetime!(2026-01-01 12:00 UTC));
        assert_eq!(span.ended_at, Some(datetime!(2026-01-01 12:30 UTC)));
        // Sum: (12:00 → 12:14:30 = 870s) + (12:15 → 12:30 = 900s)
        assert_eq!(span.full_runtime_seconds, 870 + 900);
    }

    #[test]
    fn large_gap_does_not_merge() {
        // Two same-app rows with a 5-minute gap; default 60s threshold.
        let newer = r(
            2,
            1,
            datetime!(2026-01-01 12:10 UTC),
            datetime!(2026-01-01 12:20 UTC),
        );
        let older = r(
            1,
            1,
            datetime!(2026-01-01 12:00 UTC),
            datetime!(2026-01-01 12:05 UTC),
        );
        let merged = merge_adjacent_recent(vec![newer, older], Duration::from_mins(1));
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn three_in_a_row_collapse_into_one() {
        // Each gap is 10s; threshold 60s.
        let a = r(
            3,
            1,
            datetime!(2026-01-01 12:30 UTC),
            datetime!(2026-01-01 12:40 UTC),
        );
        let b = r(
            2,
            1,
            datetime!(2026-01-01 12:20 UTC),
            datetime!(2026-01-01 12:29:50 UTC),
        );
        let c = r(
            1,
            1,
            datetime!(2026-01-01 12:00 UTC),
            datetime!(2026-01-01 12:19:50 UTC),
        );
        let merged = merge_adjacent_recent(vec![a, b, c], Duration::from_mins(1));
        assert_eq!(merged.len(), 1);
        let (span, frags) = &merged[0];
        assert_eq!(frags, &vec![3, 2, 1]);
        assert_eq!(span.started_at, datetime!(2026-01-01 12:00 UTC));
        assert_eq!(span.ended_at, Some(datetime!(2026-01-01 12:40 UTC)));
    }

    #[test]
    fn open_session_can_absorb_an_older_closed_fragment() {
        // The newest row is open (ended_at = None). A fragment that
        // closed 30s before the open one started should still merge —
        // matches "user closed the game and immediately reopened it".
        let open = r_open(2, 1, datetime!(2026-01-01 12:30 UTC));
        let older = r(
            1,
            1,
            datetime!(2026-01-01 12:00 UTC),
            datetime!(2026-01-01 12:29:30 UTC),
        );
        let merged = merge_adjacent_recent(vec![open, older], Duration::from_mins(1));
        assert_eq!(merged.len(), 1);
        let (span, frags) = &merged[0];
        assert_eq!(frags.len(), 2);
        assert_eq!(span.started_at, datetime!(2026-01-01 12:00 UTC));
        assert!(span.ended_at.is_none(), "merged span stays open");
    }

    #[test]
    fn cross_app_break_resets_the_fold() {
        // Same app, then a different app, then back to the first —
        // the second hop must not fuse across the foreign row even
        // though gaps are small.
        let rows = vec![
            r(
                3,
                1,
                datetime!(2026-01-01 12:30 UTC),
                datetime!(2026-01-01 12:40 UTC),
            ),
            r(
                2,
                2,
                datetime!(2026-01-01 12:25 UTC),
                datetime!(2026-01-01 12:29 UTC),
            ),
            r(
                1,
                1,
                datetime!(2026-01-01 12:00 UTC),
                datetime!(2026-01-01 12:24 UTC),
            ),
        ];
        let merged = merge_adjacent_recent(rows, Duration::from_mins(1));
        assert_eq!(merged.len(), 3);
        assert!(merged.iter().all(|(_, frags)| frags.len() == 1));
    }

    #[test]
    fn zero_gap_only_merges_touching_rows() {
        // Threshold zero accepts only end == start. A 1-second gap
        // is enough to keep the rows separate.
        let touching_newer = r(
            2,
            1,
            datetime!(2026-01-01 12:10 UTC),
            datetime!(2026-01-01 12:20 UTC),
        );
        let touching_older = r(
            1,
            1,
            datetime!(2026-01-01 12:00 UTC),
            datetime!(2026-01-01 12:10 UTC),
        );
        let merged = merge_adjacent_recent(vec![touching_newer, touching_older], Duration::ZERO);
        assert_eq!(merged.len(), 1, "touching rows still merge with zero gap");
    }
}
