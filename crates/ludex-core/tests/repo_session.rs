//! Integration tests for [`SessionRepo`] — lifecycle, orphan
//! recovery, close-and-rollup atomicity, delete-with-recompute,
//! and the daily-playtime aggregation queries.
//!
//! [`SessionRepo`]: ludex_core::repo::SessionRepo

mod common;

use common::sample_new_app;
use ludex_core::{
    Database, ExitReason, GameKey, PlaybackDelta, RuntimeSnapshot,
};
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

#[tokio::test]
async fn list_orphans_returns_only_stale_open_sessions() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    // Two distinct applications: `one_open_session_per_app` enforces
    // that a single app has at most one open session at a time. The
    // orphan-recovery case this test guards is per-application, so we
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

    // Fresh session: heartbeat 30 seconds ago. Should NOT be recovered.
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

    let cutoff = now - Duration::minutes(2);
    let orphans = sessions.list_orphans(cutoff).await.unwrap();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].id, stale.id);

    // Fresh session must still be open.
    let fresh_after = sessions.find_by_id(fresh.id).await.unwrap().unwrap();
    assert_eq!(fresh_after.ended_at, None);
    assert_eq!(fresh_after.exit_reason, None);
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

/// Deleting a non-existent session is a quiet no-op rather than an
/// error — keeps the GUI's "click delete twice in quick succession"
/// path simple.
#[tokio::test]
async fn delete_and_recompute_missing_id_is_no_op() {
    let db = Database::open_memory().await.unwrap();
    let deleted = db.sessions().delete_and_recompute(99_999).await.unwrap();
    assert!(!deleted);
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

/// Daily aggregation buckets by `DATE(started_at)` in UTC, sums
/// per-session runtimes, counts rows, and returns the days in
/// chronological order. The result skips days that have no sessions,
/// per the contract documented on [`SessionRepo::daily_playtime_since`].
///
/// [`SessionRepo::daily_playtime_since`]: ludex_core::repo::SessionRepo::daily_playtime_since
#[tokio::test]
async fn daily_playtime_since_buckets_by_calendar_day() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();
    let app = apps.create(sample_new_app()).await.unwrap();

    // Two sessions on day A, one on day B, one on day C. A fourth
    // session predates the cutoff and must not appear.
    let day_a = datetime!(2026-03-10 08:00:00 UTC);
    let day_b = datetime!(2026-03-11 09:00:00 UTC);
    let day_c = datetime!(2026-03-12 10:00:00 UTC);
    let pre_cutoff = datetime!(2026-03-01 10:00:00 UTC);

    for (start, full, interactive) in [
        (day_a, 300_i64, 250_i64),
        (day_a + Duration::hours(6), 600, 500),
        (day_b, 120, 120),
        (day_c, 7_200, 6_000),
        (pre_cutoff, 10_000, 10_000),
    ] {
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

    let cutoff = datetime!(2026-03-10 00:00:00 UTC);
    let rows = sessions.daily_playtime_since(cutoff).await.unwrap();

    assert_eq!(rows.len(), 3, "three distinct days at or after cutoff");
    assert_eq!(rows[0].date, "2026-03-10");
    assert_eq!(rows[0].session_count, 2);
    assert_eq!(rows[0].full_runtime_seconds, 900);
    assert_eq!(rows[0].interactive_runtime_seconds, 750);
    assert_eq!(rows[1].date, "2026-03-11");
    assert_eq!(rows[1].session_count, 1);
    assert_eq!(rows[1].full_runtime_seconds, 120);
    assert_eq!(rows[2].date, "2026-03-12");
    assert_eq!(rows[2].session_count, 1);
    assert_eq!(rows[2].full_runtime_seconds, 7_200);
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
