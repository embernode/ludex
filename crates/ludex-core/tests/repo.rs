//! Integration tests for `ludex-core` against an in-memory SQLite database.

use ludex_core::{
    Database, ExitReason, GameKey, GraphicsPlatform, Icons, IdentityUpdate, LauncherType,
    NewApplication, PlaybackDelta, ProcessArchitecture, RuntimeSnapshot,
};
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
