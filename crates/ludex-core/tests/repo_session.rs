//! Integration tests for [`SessionRepo`] — lifecycle, orphan
//! recovery, close-and-rollup atomicity, delete-with-recompute,
//! and the daily-playtime aggregation queries.
//!
//! [`SessionRepo`]: ludex_core::repo::SessionRepo

mod common;

use common::sample_new_app;
use ludex_core::{Database, ExitReason, GameKey, PlaybackDelta, RuntimeSnapshot};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

#[tokio::test]
async fn session_begin_heartbeat_end_and_playback_delta() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();

    let app = apps.create(sample_new_app()).await.unwrap();
    let start = OffsetDateTime::now_utc();

    let session = sessions.begin(app.id, start).await.unwrap();
    assert!(session.ended_at.is_none());
    assert_eq!(session.exit_reason, None);
    assert_eq!(session.full_runtime_seconds, 0);

    sessions
        .heartbeat(
            session.id,
            RuntimeSnapshot {
                full_runtime_seconds: 60,
                interactive_runtime_seconds: 45,
                at: start + Duration::seconds(60),
            },
        )
        .await
        .unwrap();

    let mid = sessions.find_by_id(session.id).await.unwrap().unwrap();
    assert_eq!(mid.full_runtime_seconds, 60);
    assert_eq!(mid.interactive_runtime_seconds, 45);
    assert!(mid.ended_at.is_none());

    let end = start + Duration::seconds(180);
    sessions
        .end(
            session.id,
            RuntimeSnapshot {
                full_runtime_seconds: 180,
                interactive_runtime_seconds: 150,
                at: end,
            },
            ExitReason::Terminated,
        )
        .await
        .unwrap();

    apps.apply_playback(
        app.id,
        PlaybackDelta {
            full_runtime_seconds: 180,
            interactive_runtime_seconds: 150,
            longest_full_candidate: Some(180),
            last_played_at: end,
        },
    )
    .await
    .unwrap();

    let after_app = apps.find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after_app.stat_run_count, 1);
    assert_eq!(after_app.stat_total_full, 180);
    assert_eq!(after_app.stat_total_interactive, 150);
    assert_eq!(after_app.stat_longest_full, 180);
    assert_eq!(after_app.last_played_at, Some(end));

    let closed = sessions.find_by_id(session.id).await.unwrap().unwrap();
    assert_eq!(closed.ended_at, Some(end));
    assert_eq!(closed.exit_reason, Some(ExitReason::Terminated));
}

/// At cold start every open session is an orphan from a dead prior
/// process — the single-instance lock guarantees no live writer — so
/// `list_all_orphans` returns them all regardless of heartbeat age. A
/// session whose heartbeat is only seconds old (the crash-then-restart
/// case) must be included, not filtered out.
#[tokio::test]
async fn list_all_orphans_returns_every_open_session() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    // Two distinct applications: `one_open_session_per_app` enforces
    // that a single app has at most one open session at a time, so we
    // set up one stale-app and one fresh-app rather than forcing two
    // sessions under the same app id.
    let stale_app = apps.create(sample_new_app()).await.unwrap();
    let mut fresh_new = sample_new_app();
    fresh_new.launcher_id = "730".into();
    fresh_new.product_name = "Counter-Strike 2".into();
    let fresh_app = apps.create(fresh_new).await.unwrap();

    let now = OffsetDateTime::now_utc();

    // Orphaned session: last heartbeat 10 minutes ago.
    let stale = sessions
        .begin(stale_app.id, now - Duration::minutes(15))
        .await
        .unwrap();
    sessions
        .heartbeat(
            stale.id,
            RuntimeSnapshot {
                full_runtime_seconds: 300,
                interactive_runtime_seconds: 250,
                at: now - Duration::minutes(10),
            },
        )
        .await
        .unwrap();

    // Fresh session: heartbeat 30 seconds ago — must ALSO be listed,
    // because at cold start it is just as orphaned as the stale one.
    let fresh = sessions
        .begin(fresh_app.id, now - Duration::seconds(60))
        .await
        .unwrap();
    sessions
        .heartbeat(
            fresh.id,
            RuntimeSnapshot {
                full_runtime_seconds: 30,
                interactive_runtime_seconds: 30,
                at: now - Duration::seconds(30),
            },
        )
        .await
        .unwrap();

    let orphans = sessions.list_all_orphans().await.unwrap();
    let ids: std::collections::HashSet<i64> = orphans.iter().map(|s| s.id).collect();
    assert_eq!(
        orphans.len(),
        2,
        "both open sessions are orphans at cold start"
    );
    assert!(ids.contains(&stale.id));
    assert!(ids.contains(&fresh.id));
}

/// Closing a session through `close_and_rollup` must update both the
/// `sessions` row and the owning `applications` aggregate stats in a
/// single transaction. This is the invariant the audit flagged: the
/// prior two-call path could drop the app-level rollup if the daemon
/// crashed between the session-close and the aggregate update.
#[tokio::test]
async fn close_and_rollup_updates_app_stats_atomically() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();
    assert_eq!(app.stat_run_count, 0);
    assert_eq!(app.stat_total_full, 0);

    let start = OffsetDateTime::now_utc() - Duration::minutes(10);
    let session = sessions.begin(app.id, start).await.unwrap();
    let end = start + Duration::seconds(300);

    sessions
        .close_and_rollup(
            session.id,
            app.id,
            RuntimeSnapshot {
                full_runtime_seconds: 300,
                interactive_runtime_seconds: 240,
                at: end,
            },
            ExitReason::Terminated,
        )
        .await
        .unwrap();

    let session_after = sessions.find_by_id(session.id).await.unwrap().unwrap();
    assert_eq!(session_after.ended_at, Some(end));
    assert_eq!(session_after.exit_reason, Some(ExitReason::Terminated));
    assert_eq!(session_after.full_runtime_seconds, 300);
    assert_eq!(session_after.interactive_runtime_seconds, 240);

    let app_after = apps.find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(app_after.stat_run_count, 1);
    assert_eq!(app_after.stat_total_full, 300);
    assert_eq!(app_after.stat_total_interactive, 240);
    assert_eq!(app_after.stat_longest_full, 300);
    assert_eq!(app_after.last_played_at, Some(end));
}

/// Deleting a closed session via `delete_and_recompute` must remove
/// the row and rebuild the owning application's denormalized stats
/// from the rows that remain. The rebuild — rather than a subtract —
/// is what keeps `stat_longest_full` correct when the deleted
/// session was the previous longest, and what prevents drift after
/// repeated deletes.
#[tokio::test]
async fn delete_and_recompute_rebuilds_aggregate_stats() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // Three sessions: a long one (the future "longest"), a medium,
    // a short. We'll delete the long one and watch
    // `stat_longest_full` drop to the medium.
    let runs: [(OffsetDateTime, i64, i64); 3] = [
        (datetime!(2026-03-01 10:00:00 UTC), 7_200, 6_000), // 2h longest
        (datetime!(2026-03-02 10:00:00 UTC), 1_800, 1_500),
        (datetime!(2026-03-03 10:00:00 UTC), 600, 500),
    ];
    let mut ids = Vec::new();
    for (start, full, interactive) in runs {
        let s = sessions.begin(app.id, start).await.unwrap();
        sessions
            .close_and_rollup(
                s.id,
                app.id,
                RuntimeSnapshot {
                    full_runtime_seconds: full,
                    interactive_runtime_seconds: interactive,
                    at: start + Duration::seconds(full),
                },
                ExitReason::Terminated,
            )
            .await
            .unwrap();
        ids.push(s.id);
    }

    // Pre-delete sanity: 3 runs, longest = 7200, totals match.
    let before = apps.find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(before.stat_run_count, 3);
    assert_eq!(before.stat_total_full, 7_200 + 1_800 + 600);
    assert_eq!(before.stat_longest_full, 7_200);

    // Delete the long one.
    let deleted = sessions.delete_and_recompute(ids[0]).await.unwrap();
    assert!(deleted, "delete_and_recompute reports the row was removed");

    let after = apps.find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.stat_run_count, 2);
    assert_eq!(after.stat_total_full, 1_800 + 600);
    assert_eq!(after.stat_total_interactive, 1_500 + 500);
    assert_eq!(
        after.stat_longest_full, 1_800,
        "longest must reflect the surviving rows, not a stale max",
    );
    assert_eq!(
        after.last_played_at,
        Some(datetime!(2026-03-03 10:10:00 UTC)),
        "last_played_at points at the surviving newest session's end",
    );

    // The session row itself is gone.
    assert!(sessions.find_by_id(ids[0]).await.unwrap().is_none());
}

/// Regression guard for PERSIST-1: `delete_and_recompute` must rebuild
/// the aggregate stats from *closed* sessions only. If it folds in an
/// unrelated open session's in-flight runtime, `close_and_rollup` adds
/// that same runtime again when the session ends — permanently
/// inflating the D-Bus-surfaced total playtime.
#[tokio::test]
async fn delete_and_recompute_excludes_open_session_runtime_from_rebuild() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // A closed session we will delete.
    let closed_start = datetime!(2026-03-01 10:00:00 UTC);
    let closed = sessions.begin(app.id, closed_start).await.unwrap();
    sessions
        .close_and_rollup(
            closed.id,
            app.id,
            RuntimeSnapshot {
                full_runtime_seconds: 600,
                interactive_runtime_seconds: 600,
                at: closed_start + Duration::seconds(600),
            },
            ExitReason::Terminated,
        )
        .await
        .unwrap();

    // A separate session for the same app, still open and days later
    // so the merge fold never groups it with the closed one. Its live
    // heartbeat runtime is 300s.
    let open_start = datetime!(2026-03-05 10:00:00 UTC);
    let open = sessions.begin(app.id, open_start).await.unwrap();
    sessions
        .heartbeat(
            open.id,
            RuntimeSnapshot {
                full_runtime_seconds: 300,
                interactive_runtime_seconds: 300,
                at: open_start + Duration::seconds(300),
            },
        )
        .await
        .unwrap();

    // Deleting the closed session rebuilds the app aggregates. The
    // open session's in-flight runtime must NOT be folded in.
    sessions.delete_and_recompute(closed.id).await.unwrap();

    // The open session ends and rolls up its 300s exactly once.
    sessions
        .close_and_rollup(
            open.id,
            app.id,
            RuntimeSnapshot {
                full_runtime_seconds: 300,
                interactive_runtime_seconds: 300,
                at: open_start + Duration::seconds(300),
            },
            ExitReason::Terminated,
        )
        .await
        .unwrap();

    let after = apps.find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(
        after.stat_run_count, 1,
        "the open session must be counted exactly once",
    );
    assert_eq!(
        after.stat_total_full, 300,
        "only the ended session's 300s — the open row must not be pre-counted then double-added",
    );
    assert_eq!(after.stat_total_interactive, 300);
}

/// Deleting a non-existent session is a quiet no-op rather than an
/// error — keeps the GUI's "click delete twice in quick succession"
/// path simple.
#[tokio::test]
async fn delete_and_recompute_missing_id_is_no_op() {
    let db = Database::open_memory().await.unwrap();
    let deleted = db.sessions().delete_and_recompute(99_999).await.unwrap();
    assert!(!deleted);
}

/// Deleting a row inside a display-merged span removes every
/// fragment in that span — the GUI shows merged sessions as a single
/// row and the user clicks delete on what they see. Same merge
/// threshold the daemon uses for display (60 s).
#[tokio::test]
async fn delete_and_recompute_drops_whole_merged_span() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // Three closed fragments 10 s apart — a merged span. Plus a
    // standalone session 5 minutes later that must survive.
    let frags: [(OffsetDateTime, i64); 3] = [
        (datetime!(2026-03-01 10:00:00 UTC), 60),
        (datetime!(2026-03-01 10:01:10 UTC), 60),
        (datetime!(2026-03-01 10:02:20 UTC), 60),
    ];
    let mut frag_ids = Vec::new();
    for (start, full) in frags {
        let s = sessions.begin(app.id, start).await.unwrap();
        sessions
            .close_and_rollup(
                s.id,
                app.id,
                RuntimeSnapshot {
                    full_runtime_seconds: full,
                    interactive_runtime_seconds: full,
                    at: start + Duration::seconds(full),
                },
                ExitReason::ForegroundChanged,
            )
            .await
            .unwrap();
        frag_ids.push(s.id);
    }
    let standalone_start = datetime!(2026-03-01 10:10:00 UTC);
    let standalone = sessions.begin(app.id, standalone_start).await.unwrap();
    sessions
        .close_and_rollup(
            standalone.id,
            app.id,
            RuntimeSnapshot {
                full_runtime_seconds: 120,
                interactive_runtime_seconds: 120,
                at: standalone_start + Duration::seconds(120),
            },
            ExitReason::Terminated,
        )
        .await
        .unwrap();

    // Delete via the *middle* fragment — the call must still drop
    // all three fragments of the span.
    let deleted = sessions.delete_and_recompute(frag_ids[1]).await.unwrap();
    assert!(deleted);

    // All three fragments gone; standalone survives.
    for fid in &frag_ids {
        assert!(
            sessions.find_by_id(*fid).await.unwrap().is_none(),
            "fragment {fid} should have been deleted as part of the merged span",
        );
    }
    assert!(sessions.find_by_id(standalone.id).await.unwrap().is_some());

    // Stats reflect only the surviving standalone row.
    let after = apps.find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(after.stat_run_count, 1);
    assert_eq!(after.stat_total_full, 120);
    assert_eq!(after.stat_longest_full, 120);
}

/// A merged span whose newest fragment is the currently-running
/// (open) session must refuse deletion: silently dropping an open
/// row would lose runtime the session manager is actively writing
/// to. Same spirit as the open-session-refusal check at the top of
/// `delete_and_recompute`, just at the span level.
#[tokio::test]
async fn delete_and_recompute_refuses_merged_span_with_open_fragment() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // Closed older fragment, then an open newer one within the
    // merge gap — the fold absorbs the older into the open head.
    let older_start = datetime!(2026-03-01 10:00:00 UTC);
    let older = sessions.begin(app.id, older_start).await.unwrap();
    sessions
        .close_and_rollup(
            older.id,
            app.id,
            RuntimeSnapshot {
                full_runtime_seconds: 60,
                interactive_runtime_seconds: 60,
                at: older_start + Duration::seconds(60),
            },
            ExitReason::ForegroundChanged,
        )
        .await
        .unwrap();
    let open = sessions
        .begin(app.id, datetime!(2026-03-01 10:01:10 UTC))
        .await
        .unwrap();

    let result = sessions.delete_and_recompute(older.id).await;
    assert!(
        matches!(result, Err(ludex_core::Error::Invariant(_))),
        "deleting a closed fragment whose merged span includes an open one must error",
    );
    // Both rows still present — the refusal must protect both the
    // requested closed fragment and the live open fragment.
    assert!(sessions.find_by_id(older.id).await.unwrap().is_some());
    assert!(sessions.find_by_id(open.id).await.unwrap().is_some());
}

/// Refusing to delete an open session protects in-flight runtime
/// from being silently dropped. The session manager is the
/// authoritative writer for an open row; the GUI must stop the
/// game before deleting.
#[tokio::test]
async fn delete_and_recompute_refuses_open_session() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    let open = sessions
        .begin(app.id, OffsetDateTime::now_utc())
        .await
        .unwrap();
    let result = sessions.delete_and_recompute(open.id).await;
    assert!(matches!(result, Err(ludex_core::Error::Invariant(_))));

    // Session row must still exist after the refused delete.
    assert!(sessions.find_by_id(open.id).await.unwrap().is_some());
}

/// Resolve the local calendar day SQLite assigns to `ts` — the same
/// `DATE(…, 'localtime')` conversion `daily_playtime_since` uses — so
/// expected buckets stay correct in whatever timezone the test host
/// runs. CI pins `TZ=Europe/Helsinki` so a non-UTC offset is actually
/// exercised there rather than degenerating to the UTC identity.
async fn local_date(db: &Database, ts: OffsetDateTime) -> String {
    sqlx::query_scalar("SELECT DATE(?, 'localtime')")
        .bind(ts)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

/// Daily aggregation buckets by the *local* calendar day
/// (`DATE(started_at, 'localtime')`), sums per-session runtimes,
/// counts rows, and returns the days in chronological order. The
/// result skips days that have no sessions, per the contract
/// documented on [`SessionRepo::daily_playtime_since`].
///
/// [`SessionRepo::daily_playtime_since`]: ludex_core::repo::SessionRepo::daily_playtime_since
#[tokio::test]
async fn daily_playtime_since_buckets_by_local_calendar_day() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // Two sessions on day A, one on day B, one on day C (all mid-day
    // UTC). A fourth session predates the cutoff and must not appear.
    let day_a = datetime!(2026-03-10 08:00:00 UTC);
    let day_b = datetime!(2026-03-11 09:00:00 UTC);
    let day_c = datetime!(2026-03-12 10:00:00 UTC);
    let pre_cutoff = datetime!(2026-03-01 10:00:00 UTC);

    let counted = [
        (day_a, 300_i64, 250_i64),
        (day_a + Duration::hours(6), 600, 500),
        (day_b, 120, 120),
        (day_c, 7_200, 6_000),
    ];
    for (start, full, interactive) in counted
        .iter()
        .copied()
        .chain([(pre_cutoff, 10_000, 10_000)])
    {
        let s = sessions.begin(app.id, start).await.unwrap();
        sessions
            .close_and_rollup(
                s.id,
                app.id,
                RuntimeSnapshot {
                    full_runtime_seconds: full,
                    interactive_runtime_seconds: interactive,
                    at: start + Duration::seconds(full),
                },
                ExitReason::Terminated,
            )
            .await
            .unwrap();
    }

    // Expected buckets, keyed by the local day each start falls on.
    // BTreeMap iteration order is ascending, matching the query's
    // ORDER BY.
    let mut expected: std::collections::BTreeMap<String, (i64, i64, i64)> =
        std::collections::BTreeMap::new();
    for (start, full, interactive) in counted {
        let e = expected.entry(local_date(&db, start).await).or_default();
        e.0 += 1;
        e.1 += full;
        e.2 += interactive;
    }

    let cutoff = datetime!(2026-03-10 00:00:00 UTC);
    let rows = sessions.daily_playtime_since(cutoff).await.unwrap();

    assert_eq!(rows.len(), expected.len(), "one row per distinct local day");
    for (row, (date, (count, full, interactive))) in rows.iter().zip(expected.iter()) {
        assert_eq!(&row.date, date);
        assert_eq!(row.session_count, *count);
        assert_eq!(row.full_runtime_seconds, *full);
        assert_eq!(row.interactive_runtime_seconds, *interactive);
    }
}

/// Sessions either side of a UTC midnight that share a local calendar
/// day must land in one bucket. Guards the dashboard regression where
/// an evening session was attributed to the next day for every user
/// east of UTC (the daemon bucketed by UTC day). Vacuous under
/// `TZ=UTC`; CI pins `TZ=Europe/Helsinki` to keep it meaningful.
#[tokio::test]
async fn daily_playtime_since_groups_by_local_not_utc_day() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // 23:30 UTC and 01:30 UTC — different UTC days, one local day
    // anywhere with a positive offset of 30 minutes or more.
    let before_utc_midnight = datetime!(2026-03-10 23:30:00 UTC);
    let after_utc_midnight = datetime!(2026-03-11 01:30:00 UTC);

    for start in [before_utc_midnight, after_utc_midnight] {
        let s = sessions.begin(app.id, start).await.unwrap();
        sessions
            .close_and_rollup(
                s.id,
                app.id,
                RuntimeSnapshot {
                    full_runtime_seconds: 300,
                    interactive_runtime_seconds: 300,
                    at: start + Duration::seconds(300),
                },
                ExitReason::Terminated,
            )
            .await
            .unwrap();
    }

    let expected_days: std::collections::BTreeSet<String> = [
        local_date(&db, before_utc_midnight).await,
        local_date(&db, after_utc_midnight).await,
    ]
    .into_iter()
    .collect();

    let rows = sessions
        .daily_playtime_since(datetime!(2026-03-09 00:00:00 UTC))
        .await
        .unwrap();

    assert_eq!(rows.len(), expected_days.len());
    for (row, day) in rows.iter().zip(expected_days.iter()) {
        assert_eq!(&row.date, day);
    }
    let total_sessions: i64 = rows.iter().map(|r| r.session_count).sum();
    let total_full: i64 = rows.iter().map(|r| r.full_runtime_seconds).sum();
    assert_eq!(total_sessions, 2);
    assert_eq!(total_full, 600);
}

/// Regression for PERSIST-4: the cutoff must be compared at local-day
/// granularity to match the local-day `GROUP BY`, not as a raw instant.
/// A session earlier on the cutoff's own local day — before the cutoff
/// instant's time-of-day — must still be counted; otherwise the oldest
/// bucket silently undercounts every session logged before "now o'clock"
/// on that day. Exercises the bug in any timezone (the two times share a
/// local day under UTC and every moderate offset), so it is not vacuous
/// under `TZ=UTC`.
#[tokio::test]
async fn daily_playtime_since_counts_full_cutoff_local_day() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // Cutoff at mid-day; an earlier session the SAME local day (two hours
    // before the cutoff instant) must count. A session on the prior local
    // day must stay excluded.
    let cutoff = datetime!(2026-03-10 12:00:00 UTC);
    let same_day_earlier = datetime!(2026-03-10 10:00:00 UTC);
    let prior_day = datetime!(2026-03-09 10:00:00 UTC);

    for (start, full) in [(same_day_earlier, 300_i64), (prior_day, 999)] {
        let s = sessions.begin(app.id, start).await.unwrap();
        sessions
            .close_and_rollup(
                s.id,
                app.id,
                RuntimeSnapshot {
                    full_runtime_seconds: full,
                    interactive_runtime_seconds: full,
                    at: start + Duration::seconds(full),
                },
                ExitReason::Terminated,
            )
            .await
            .unwrap();
    }

    let rows = sessions.daily_playtime_since(cutoff).await.unwrap();
    let cutoff_day = local_date(&db, same_day_earlier).await;
    let prior = local_date(&db, prior_day).await;

    let bucket = rows
        .iter()
        .find(|r| r.date == cutoff_day)
        .expect("the cutoff's own local day must be present");
    assert_eq!(
        bucket.full_runtime_seconds, 300,
        "a session earlier on the cutoff day must be counted, not dropped by an instant compare",
    );
    assert!(
        rows.iter().all(|r| r.date != prior),
        "the prior local day is before the cutoff and stays excluded",
    );
}

/// Blocking an application removes its sessions from the
/// dashboard aggregates — matches the behaviour users see in the
/// Games + Recent GUI views. Covers the dashboard-shows-blocked
/// bug the "hide blocked from views" commit didn't catch.
#[tokio::test]
async fn daily_playtime_since_excludes_sessions_from_blocked_apps() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();

    // Two applications, each with a session on the same day.
    let kept = apps.create(sample_new_app()).await.unwrap();
    let mut blocked_new = sample_new_app();
    blocked_new.launcher_id = "999".into();
    blocked_new.product_name = "To Block".into();
    let blocked = apps.create(blocked_new).await.unwrap();

    let at = datetime!(2026-04-10 12:00:00 UTC);
    for (app_id, seconds) in [(kept.id, 600_i64), (blocked.id, 3_600_i64)] {
        let s = sessions.begin(app_id, at).await.unwrap();
        sessions
            .close_and_rollup(
                s.id,
                app_id,
                RuntimeSnapshot {
                    full_runtime_seconds: seconds,
                    interactive_runtime_seconds: seconds,
                    at: at + Duration::seconds(seconds),
                },
                ExitReason::Terminated,
            )
            .await
            .unwrap();
    }

    // Baseline: both apps contribute.
    let before = sessions
        .daily_playtime_since(datetime!(2026-04-01 00:00:00 UTC))
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].full_runtime_seconds, 4_200);
    assert_eq!(before[0].session_count, 2);

    // Block the noisier app; aggregate drops to just the kept one.
    let blocked_key = GameKey::new(blocked.launcher_type, blocked.launcher_id.clone());
    db.blocked()
        .insert(&blocked_key, OffsetDateTime::now_utc())
        .await
        .unwrap();

    let after = sessions
        .daily_playtime_since(datetime!(2026-04-01 00:00:00 UTC))
        .await
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].full_runtime_seconds, 600);
    assert_eq!(after[0].session_count, 1);

    // Unblocking brings it back on the next call — no cache to
    // invalidate, the subquery re-runs.
    db.blocked().remove(&blocked_key).await.unwrap();
    let restored = sessions
        .daily_playtime_since(datetime!(2026-04-01 00:00:00 UTC))
        .await
        .unwrap();
    assert_eq!(restored[0].full_runtime_seconds, 4_200);
    assert_eq!(restored[0].session_count, 2);
}

#[tokio::test]
async fn daily_playtime_since_empty_when_no_sessions_match() {
    let db = Database::open_memory().await.unwrap();
    let cutoff = datetime!(2026-03-10 00:00:00 UTC);
    let rows = db.sessions().daily_playtime_since(cutoff).await.unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn check_constraint_rejects_interactive_exceeding_full() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();
    let session = sessions
        .begin(app.id, OffsetDateTime::now_utc())
        .await
        .unwrap();

    let err = sessions
        .heartbeat(
            session.id,
            RuntimeSnapshot {
                full_runtime_seconds: 10,
                interactive_runtime_seconds: 20, // invalid: > full
                at: OffsetDateTime::now_utc(),
            },
        )
        .await
        .expect_err("CHECK constraint should reject");
    assert!(err.to_string().contains("CHECK"), "got: {err}");
}

/// A second `close_and_rollup` on an already-closed session must be a
/// no-op. The session `UPDATE` is guarded by `ended_at IS NULL`, so it
/// matches nothing — and the application rollup must then be skipped
/// too, or a double close double-counts `stat_run_count` and the
/// runtime totals.
#[tokio::test]
async fn close_and_rollup_is_noop_when_session_already_closed() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    let start = OffsetDateTime::now_utc() - Duration::minutes(10);
    let session = sessions.begin(app.id, start).await.unwrap();
    let snapshot = RuntimeSnapshot {
        full_runtime_seconds: 300,
        interactive_runtime_seconds: 240,
        at: start + Duration::seconds(300),
    };

    sessions
        .close_and_rollup(session.id, app.id, snapshot, ExitReason::Terminated)
        .await
        .unwrap();
    sessions
        .close_and_rollup(session.id, app.id, snapshot, ExitReason::Terminated)
        .await
        .unwrap();

    let app_after = apps.find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(
        app_after.stat_run_count, 1,
        "double close must not double-count"
    );
    assert_eq!(app_after.stat_total_full, 300);
    assert_eq!(app_after.stat_total_interactive, 240);
    assert_eq!(app_after.last_played_at, Some(snapshot.at));
}
