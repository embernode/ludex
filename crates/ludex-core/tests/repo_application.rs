//! Integration tests for [`ApplicationRepo`] and the database-open
//! path that backs it.
//!
//! [`ApplicationRepo`]: ludex_core::repo::ApplicationRepo

mod common;

use common::sample_new_app;
use ludex_core::{
    Database, ExitReason, GameKey, GraphicsPlatform, IdentityUpdate, LauncherType,
    PlaybackDelta, RuntimeSnapshot,
};
use time::{Duration, OffsetDateTime};

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
