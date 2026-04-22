//! Integration tests for [`SessionManager`].
//!
//! Drive the manager with synthetic events and assert the resulting
//! database state — no sources involved.

use std::sync::Arc;
use std::time::Duration;

use ludex_core::{Database, ExitReason, GameKey};
use ludex_daemon::{GameEvent, SessionManager};
use ludex_enrich::EnrichmentContext;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch};

fn default_enrichment_ctx() -> Arc<EnrichmentContext> {
    Arc::new(EnrichmentContext::default())
}

async fn yield_for(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Drive the manager through a full Started→Stopped lifecycle and verify
/// that the application, session, and aggregate stats rows match.
#[tokio::test]
async fn started_then_stopped_creates_one_closed_session() {
    let db = Database::open_memory().await.unwrap();
    let manager = SessionManager::new(db.clone(), default_enrichment_ctx());
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(manager.run(rx, shutdown_rx));

    let at = OffsetDateTime::now_utc();
    tx.send(GameEvent::Started {
        key: GameKey::steam("440"),
        display_name: "Team Fortress 2".into(),
        at,
    })
    .await
    .unwrap();
    yield_for(50).await;

    let app = db
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .expect("application inserted");
    assert_eq!(app.product_name, "Team Fortress 2");
    assert_eq!(app.stat_run_count, 0); // not yet incremented — session still open

    let open_sessions = db
        .sessions()
        .list_for_application(app.id, 10)
        .await
        .unwrap();
    assert_eq!(open_sessions.len(), 1);
    assert!(open_sessions[0].ended_at.is_none());

    tx.send(GameEvent::Stopped {
        key: GameKey::steam("440"),
        at: at + time::Duration::seconds(120),
    })
    .await
    .unwrap();
    yield_for(50).await;

    let closed = db
        .sessions()
        .list_for_application(app.id, 10)
        .await
        .unwrap();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].ended_at.is_some());
    assert_eq!(closed[0].exit_reason, Some(ExitReason::Terminated));
    assert_eq!(closed[0].full_runtime_seconds, 120);

    let refreshed = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(refreshed.stat_run_count, 1);
    assert_eq!(refreshed.stat_total_full, 120);
    assert_eq!(refreshed.stat_longest_full, 120);

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// A duplicate `Started` for an already-open key must not create a second
/// session; the manager logs a warning and drops the event.
#[tokio::test]
async fn duplicate_started_is_ignored() {
    let db = Database::open_memory().await.unwrap();
    let manager = SessionManager::new(db.clone(), default_enrichment_ctx());
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(manager.run(rx, shutdown_rx));
    let at = OffsetDateTime::now_utc();

    for _ in 0..2 {
        tx.send(GameEvent::Started {
            key: GameKey::steam("440"),
            display_name: "Team Fortress 2".into(),
            at,
        })
        .await
        .unwrap();
    }
    yield_for(50).await;

    let app = db
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .unwrap();
    let sessions = db
        .sessions()
        .list_for_application(app.id, 10)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1, "no duplicate session inserted");

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// Graceful shutdown while a session is open must close that session
/// with `Terminated`.
#[tokio::test]
async fn shutdown_closes_open_sessions() {
    let db = Database::open_memory().await.unwrap();
    let manager = SessionManager::new(db.clone(), default_enrichment_ctx());
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(manager.run(rx, shutdown_rx));
    let at = OffsetDateTime::now_utc();
    tx.send(GameEvent::Started {
        key: GameKey::steam("440"),
        display_name: "Team Fortress 2".into(),
        at,
    })
    .await
    .unwrap();
    yield_for(50).await;

    // Trigger shutdown without sending a Stopped event.
    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();

    let app = db
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .unwrap();
    let sessions = db
        .sessions()
        .list_for_application(app.id, 10)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].exit_reason, Some(ExitReason::Terminated));
    assert!(sessions[0].ended_at.is_some());
}

/// Orphan recovery closes sessions left open by a previous run whose
/// heartbeat is older than the grace period.
#[tokio::test]
async fn recover_orphans_closes_stale_sessions() {
    let db = Database::open_memory().await.unwrap();

    // Arrange a pre-existing open session with a stale heartbeat.
    let app = {
        use ludex_core::{GraphicsPlatform, Icons, NewApplication, ProcessArchitecture};
        db.applications()
            .create(NewApplication {
                launcher_type: ludex_core::LauncherType::Steam,
                launcher_id: "440".into(),
                product_name: "Team Fortress 2".into(),
                publisher: None,
                version: None,
                executable_path: None,
                launcher_exe_path: None,
                wineprefix_path: None,
                installed_flatpak_ref: None,
                graphics_platform: GraphicsPlatform::Unknown,
                process_architecture: ProcessArchitecture::Unknown,
                group_id: None,
                icons: Icons::default(),
                first_seen_at: OffsetDateTime::now_utc(),
            })
            .await
            .unwrap()
    };
    let now = OffsetDateTime::now_utc();
    let stale = db
        .sessions()
        .begin(app.id, now - time::Duration::minutes(30))
        .await
        .unwrap();
    db.sessions()
        .heartbeat(
            stale.id,
            ludex_core::RuntimeSnapshot {
                full_runtime_seconds: 600,
                interactive_runtime_seconds: 600,
                at: now - time::Duration::minutes(20),
            },
        )
        .await
        .unwrap();

    // Act.
    let manager = SessionManager::new(db.clone(), default_enrichment_ctx());
    let closed = manager.recover_orphans().await.unwrap();
    assert_eq!(closed, 1);

    let refreshed = db.sessions().find_by_id(stale.id).await.unwrap().unwrap();
    assert_eq!(refreshed.exit_reason, Some(ExitReason::Recovered));
    assert_eq!(refreshed.ended_at, Some(refreshed.heartbeat_at));
}

/// When a Started event creates a new application, the enrichment
/// cascade runs in the background and overwrites any placeholder name
/// with the canonical one from the configured sources.
#[tokio::test]
async fn enrichment_fires_on_new_application() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    fs::create_dir_all(&steamapps).unwrap();
    fs::write(
        steamapps.join("appmanifest_440.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\"440\"\n\t\"name\"\t\"Team Fortress 2\"\n}",
    )
    .unwrap();

    let ctx = Arc::new(EnrichmentContext {
        steam_dir: Some(tmp.path().to_path_buf()),
        ..Default::default()
    });
    let db = Database::open_memory().await.unwrap();
    let manager = SessionManager::new(db.clone(), ctx);
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(manager.run(rx, shutdown_rx));

    // Daemon initially inserts the placeholder name the source supplied.
    tx.send(GameEvent::Started {
        key: GameKey::steam("440"),
        display_name: "AppID 440".into(),
        at: OffsetDateTime::now_utc(),
    })
    .await
    .unwrap();

    // Give the spawned enrichment task time to complete.
    for _ in 0..20 {
        yield_for(25).await;
        let app = db
            .applications()
            .find_by_key(&GameKey::steam("440"))
            .await
            .unwrap();
        if matches!(app, Some(a) if a.product_name == "Team Fortress 2") {
            break;
        }
    }

    let final_app = db
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_app.product_name, "Team Fortress 2");

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}
