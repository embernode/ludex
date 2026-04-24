//! Integration tests for `ludex-core` against an in-memory SQLite database.

use ludex_core::repo::GPU_MEMORY_THRESHOLD_BYTES;
use ludex_core::{
    Database, ExitReason, GameKey, GraphicsPlatform, Icons, IdentityUpdate, LauncherType,
    NewApplication, PlaybackDelta, ProcessArchitecture, RuntimeSnapshot,
};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

fn sample_new_app() -> NewApplication {
    NewApplication {
        launcher_type: LauncherType::Steam,
        launcher_id: "440".into(),
        product_name: "Team Fortress 2".into(),
        publisher: Some("Valve".into()),
        version: None,
        executable_path: Some(
            "/home/x/.local/share/Steam/steamapps/common/Team Fortress 2/hl2_linux".into(),
        ),
        launcher_exe_path: None,
        wineprefix_path: None,
        installed_flatpak_ref: None,
        graphics_platform: GraphicsPlatform::OpenGL,
        process_architecture: ProcessArchitecture::Amd64,
        group_id: None,
        icons: Icons::default(),
        first_seen_at: OffsetDateTime::now_utc(),
    }
}

/// `Database::open` must accept filesystem paths containing characters
/// that have syntactic meaning in a URL (`?`, `#`, `%`, spaces,
/// apostrophes). A path is not a URL; the prior implementation
/// `format!("sqlite://{}", path)` broke on "Sam's Games" or any path
/// with a bare space.
#[tokio::test]
async fn open_handles_paths_with_url_special_chars() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Sam's Games");
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("ludex?test#1.sqlite");

    let db = Database::open(&db_path).await.expect("opens awkward path");
    let apps = db.applications();
    let created = apps.create(sample_new_app()).await.unwrap();
    assert_eq!(created.launcher_id, "440");

    // Close, reopen, read back — confirms the file on disk is what we
    // wrote, not a URL-mangled sibling.
    db.close().await;
    let db = Database::open(&db_path).await.unwrap();
    let found = db
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap();
    assert!(found.is_some());
}

#[tokio::test]
async fn create_and_find_by_key() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();

    let created = apps.create(sample_new_app()).await.unwrap();
    assert!(created.id > 0);
    assert_eq!(created.launcher_type, LauncherType::Steam);
    assert_eq!(created.launcher_id, "440");
    assert_eq!(created.stat_run_count, 0);

    let found = apps
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .expect("row exists");
    assert_eq!(found, created);

    let missing = apps.find_by_key(&GameKey::steam("999999")).await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn uniqueness_enforced() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();

    apps.create(sample_new_app()).await.unwrap();
    let err = apps
        .create(sample_new_app())
        .await
        .expect_err("duplicate (launcher_type, launcher_id) should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("UNIQUE") || msg.contains("constraint"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn identity_update_preserves_unset_fields() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();

    let created = apps.create(sample_new_app()).await.unwrap();

    apps.update_identity(
        created.id,
        IdentityUpdate {
            version: Some("build-1234".into()),
            graphics_platform: Some(GraphicsPlatform::Vulkan),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let after = apps.find_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(after.version.as_deref(), Some("build-1234"));
    assert_eq!(after.graphics_platform, GraphicsPlatform::Vulkan);
    // Fields not mentioned in the update are untouched.
    assert_eq!(after.product_name, created.product_name);
    assert_eq!(after.publisher, created.publisher);
}

#[tokio::test]
async fn identity_update_with_empty_patch_is_a_no_op() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let created = apps.create(sample_new_app()).await.unwrap();

    apps.update_identity(created.id, IdentityUpdate::default())
        .await
        .unwrap();

    let after = apps.find_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(after, created);
}

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
    let app = apps.create(sample_new_app()).await.unwrap();

    let now = OffsetDateTime::now_utc();

    // Orphaned session: last heartbeat 10 minutes ago.
    let stale = sessions
        .begin(app.id, now - Duration::minutes(15))
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
        .begin(app.id, now - Duration::seconds(60))
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

/// Daily aggregation buckets by `DATE(started_at)` in UTC, sums
/// per-session runtimes, counts rows, and returns the days in
/// chronological order. The result skips days that have no sessions,
/// per the contract documented on [`SessionRepo::daily_playtime_since`].
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

/// Happy-path merge: two apps with different launcher types but
/// the same underlying game. Sessions on src move to dst, stats
/// sum correctly, and src is gone from the table.
#[tokio::test]
async fn merge_into_folds_sessions_and_stats() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let sessions = db.sessions();

    // dst: a Steam-detected Core Keeper with a single short
    // session (the 18-second edge-case the real user hit).
    let dst = apps.create(sample_new_app()).await.unwrap();
    let dst_session = sessions
        .begin(dst.id, OffsetDateTime::now_utc() - Duration::minutes(5))
        .await
        .unwrap();
    sessions
        .close_and_rollup(
            dst_session.id,
            dst.id,
            RuntimeSnapshot {
                full_runtime_seconds: 18,
                interactive_runtime_seconds: 18,
                at: OffsetDateTime::now_utc() - Duration::minutes(4),
            },
            ExitReason::Terminated,
        )
        .await
        .unwrap();

    // src: a migrated row under Native. Different launcher id so
    // the primary-key uniqueness isn't an issue.
    let mut src_new = sample_new_app();
    src_new.launcher_type = LauncherType::Native;
    src_new.launcher_id = "/pelit/corekeeper.exe".into();
    src_new.publisher = Some("Pugstorm".into()); // dst was Valve; we keep dst's here
    let src = apps.create(src_new).await.unwrap();
    let src_session = sessions
        .begin(src.id, OffsetDateTime::now_utc() - Duration::days(30))
        .await
        .unwrap();
    sessions
        .close_and_rollup(
            src_session.id,
            src.id,
            RuntimeSnapshot {
                full_runtime_seconds: 7_200, // 2 hours
                interactive_runtime_seconds: 6_500,
                at: OffsetDateTime::now_utc() - Duration::days(30) + Duration::hours(2),
            },
            ExitReason::Terminated,
        )
        .await
        .unwrap();

    apps.merge_into(src.id, dst.id).await.unwrap();

    // src is gone.
    assert!(apps.find_by_id(src.id).await.unwrap().is_none());
    // dst carries both sessions.
    let dst_sessions = sessions.list_for_application(dst.id, 10).await.unwrap();
    assert_eq!(dst_sessions.len(), 2);
    // Aggregate stats reflect both histories.
    let merged = apps.find_by_id(dst.id).await.unwrap().unwrap();
    assert_eq!(merged.stat_run_count, 2);
    assert_eq!(merged.stat_total_full, 7_218);
    assert_eq!(merged.stat_total_interactive, 6_518);
    // Longest uses MAX not SUM.
    assert_eq!(merged.stat_longest_full, 7_200);
    // Identity on dst is preserved.
    assert_eq!(merged.launcher_type, dst.launcher_type);
    assert_eq!(merged.launcher_id, dst.launcher_id);
    assert_eq!(merged.product_name, dst.product_name);
    assert_eq!(merged.publisher, dst.publisher);
}

#[tokio::test]
async fn merge_into_fills_missing_metadata_from_src() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();

    // dst lacks a publisher and an icon slot.
    let mut dst_new = sample_new_app();
    dst_new.publisher = None;
    dst_new.icons.icon_32 = None;
    let dst = apps.create(dst_new).await.unwrap();

    // src has both.
    let mut src_new = sample_new_app();
    src_new.launcher_id = "/tmp/alt.exe".into();
    src_new.launcher_type = LauncherType::Native;
    src_new.publisher = Some("Backfilled Inc.".into());
    src_new.icons.icon_32 = Some(vec![0x42; 32 * 32 * 4]);
    let src = apps.create(src_new).await.unwrap();

    apps.merge_into(src.id, dst.id).await.unwrap();

    let after = apps.find_by_id(dst.id).await.unwrap().unwrap();
    assert_eq!(after.publisher.as_deref(), Some("Backfilled Inc."));
    assert_eq!(after.icon_32.as_ref().map(Vec::len), Some(32 * 32 * 4));
}

#[tokio::test]
async fn merge_into_rejects_same_id() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let app = apps.create(sample_new_app()).await.unwrap();

    let err = apps
        .merge_into(app.id, app.id)
        .await
        .expect_err("same-id merge must fail");
    assert!(err.to_string().contains("same"), "got: {err}");
}

#[tokio::test]
async fn merge_into_errors_on_missing_rows() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();
    let app = apps.create(sample_new_app()).await.unwrap();

    // Nonexistent src.
    let err = apps.merge_into(99_999, app.id).await.unwrap_err();
    assert!(err.to_string().contains("source"), "got: {err}");

    // Nonexistent dst.
    let err = apps.merge_into(app.id, 99_999).await.unwrap_err();
    assert!(err.to_string().contains("destination"), "got: {err}");
}

#[tokio::test]
async fn blocked_repo_round_trips_keys() {
    let db = Database::open_memory().await.unwrap();
    let repo = db.blocked();
    let key = GameKey::steam("440");

    assert!(!repo.contains(&key).await.unwrap());
    assert!(repo.list().await.unwrap().is_empty());

    let inserted = repo.insert(&key, OffsetDateTime::now_utc()).await.unwrap();
    assert!(inserted, "first insert returns true");
    assert!(repo.contains(&key).await.unwrap());

    // Second insert of the same key is a no-op (INSERT OR IGNORE).
    let second = repo.insert(&key, OffsetDateTime::now_utc()).await.unwrap();
    assert!(!second, "duplicate insert returns false");
    assert_eq!(repo.list().await.unwrap().len(), 1);

    let removed = repo.remove(&key).await.unwrap();
    assert!(removed);
    assert!(!repo.contains(&key).await.unwrap());

    // Removing an absent key is a no-op.
    assert!(!repo.remove(&key).await.unwrap());
}

#[tokio::test]
async fn blocked_repo_lists_every_launcher_type() {
    let db = Database::open_memory().await.unwrap();
    let repo = db.blocked();
    let now = OffsetDateTime::now_utc();

    for key in [
        GameKey::steam("440"),
        GameKey::lutris("celeste"),
        GameKey::heroic("com.example.fooo"),
        GameKey::native("/opt/games/foo/foo"),
    ] {
        repo.insert(&key, now).await.unwrap();
    }

    let set = repo.list().await.unwrap();
    assert_eq!(set.len(), 4);
    assert!(set.contains(&GameKey::steam("440")));
    assert!(set.contains(&GameKey::native("/opt/games/foo/foo")));
}

#[tokio::test]
async fn settings_get_u64_returns_fallback_when_row_absent() {
    let db = Database::open_memory().await.unwrap();
    let v = db
        .settings()
        .get_u64(GPU_MEMORY_THRESHOLD_BYTES, 123)
        .await
        .unwrap();
    assert_eq!(v, 123);
}

#[tokio::test]
async fn settings_set_then_get_round_trips() {
    let db = Database::open_memory().await.unwrap();
    db.settings()
        .set_u64(GPU_MEMORY_THRESHOLD_BYTES, 10_000_000)
        .await
        .unwrap();
    let v = db
        .settings()
        .get_u64(GPU_MEMORY_THRESHOLD_BYTES, 0)
        .await
        .unwrap();
    assert_eq!(v, 10_000_000);
}

#[tokio::test]
async fn settings_set_is_upsert() {
    let db = Database::open_memory().await.unwrap();
    let s = db.settings();
    s.set_u64(GPU_MEMORY_THRESHOLD_BYTES, 1).await.unwrap();
    s.set_u64(GPU_MEMORY_THRESHOLD_BYTES, 2).await.unwrap();
    assert_eq!(s.get_u64(GPU_MEMORY_THRESHOLD_BYTES, 99).await.unwrap(), 2);
}

#[tokio::test]
async fn settings_remove_returns_false_when_absent() {
    let db = Database::open_memory().await.unwrap();
    assert!(!db.settings().remove("nope").await.unwrap());
}

#[tokio::test]
async fn settings_set_rejects_empty_value() {
    let db = Database::open_memory().await.unwrap();
    let err = db
        .settings()
        .set_raw(GPU_MEMORY_THRESHOLD_BYTES, "")
        .await
        .expect_err("empty value should be rejected");
    assert!(err.to_string().contains("empty"), "got: {err}");
}

#[tokio::test]
async fn settings_get_u64_rejects_non_numeric() {
    let db = Database::open_memory().await.unwrap();
    db.settings()
        .set_raw(GPU_MEMORY_THRESHOLD_BYTES, "not-a-number")
        .await
        .unwrap();
    let err = db
        .settings()
        .get_u64(GPU_MEMORY_THRESHOLD_BYTES, 0)
        .await
        .expect_err("unparseable value should surface");
    assert!(err.to_string().contains("u64"), "got: {err}");
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

#[tokio::test]
async fn list_sorts_most_recent_first_nulls_last() {
    let db = Database::open_memory().await.unwrap();
    let apps = db.applications();

    let mut older = sample_new_app();
    older.launcher_id = "100".into();
    older.product_name = "Older".into();
    let older = apps.create(older).await.unwrap();

    let mut newer = sample_new_app();
    newer.launcher_id = "200".into();
    newer.product_name = "Newer".into();
    let newer = apps.create(newer).await.unwrap();

    let mut unplayed = sample_new_app();
    unplayed.launcher_id = "300".into();
    unplayed.product_name = "Never".into();
    let unplayed = apps.create(unplayed).await.unwrap();

    let now = OffsetDateTime::now_utc();
    apps.apply_playback(
        older.id,
        PlaybackDelta {
            full_runtime_seconds: 60,
            interactive_runtime_seconds: 60,
            longest_full_candidate: Some(60),
            last_played_at: now - Duration::hours(2),
        },
    )
    .await
    .unwrap();
    apps.apply_playback(
        newer.id,
        PlaybackDelta {
            full_runtime_seconds: 60,
            interactive_runtime_seconds: 60,
            longest_full_candidate: Some(60),
            last_played_at: now,
        },
    )
    .await
    .unwrap();

    let list = apps.list().await.unwrap();
    let ids: Vec<i64> = list.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![newer.id, older.id, unplayed.id]);
}
