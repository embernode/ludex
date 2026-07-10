//! Integration tests for [`SessionManager`].
//!
//! Drive the manager with synthetic events and assert the resulting
//! database state — no sources involved.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use ludex_core::{Database, ExitReason, GameKey};
use ludex_daemon::config::{BackupConfig, SharedConfig, TrackerConfig};
use ludex_daemon::gate::GateConfig;
use ludex_daemon::idle::IdleTracker;
use ludex_daemon::sleep::SleepTracker;
use ludex_daemon::{GameEvent, SessionManager, SharedBlocklist};
use ludex_enrich::EnrichmentContext;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch, RwLock};

fn default_enrichment_ctx() -> Arc<EnrichmentContext> {
    Arc::new(EnrichmentContext::default())
}

fn default_idle_tracker() -> Arc<IdleTracker> {
    Arc::new(IdleTracker::new())
}

fn default_sleep_tracker() -> Arc<SleepTracker> {
    Arc::new(SleepTracker::new())
}

fn default_config() -> SharedConfig {
    config_with_idle_grace(Duration::from_mins(5))
}

/// Same as [`default_config`] but with a caller-chosen idle grace
/// — used by tests that want to exercise the cutscene-forgiveness
/// math directly (e.g. asserting "idle under grace is fully
/// forgiven" or "long idle is billed minus grace").
fn config_with_idle_grace(grace: Duration) -> SharedConfig {
    Arc::new(RwLock::new(TrackerConfig {
        gate: GateConfig::default(),
        alt_tab_grace: Duration::from_secs(15),
        pause_when_backgrounded: true,
        idle_grace: grace,
        backup: BackupConfig {
            interval: Duration::from_hours(24),
            retention: 14,
        },
    }))
}

fn empty_blocklist() -> SharedBlocklist {
    Arc::new(RwLock::new(HashSet::new()))
}

fn blocklist_with(keys: impl IntoIterator<Item = GameKey>) -> SharedBlocklist {
    Arc::new(RwLock::new(keys.into_iter().collect()))
}

async fn yield_for(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// Drive the manager through a full Started→Stopped lifecycle and verify
/// that the application, session, and aggregate stats rows match.
#[tokio::test]
async fn started_then_stopped_creates_one_closed_session() {
    let db = Database::open_memory().await.unwrap();
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        empty_blocklist(),
    );
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

/// A blocked key (mirrored from the `blocked_applications` table into
/// `SharedBlocklist`) must never open a session — the session
/// manager drops the Started event before creating an application
/// or opening a row. Already-tracked applications from before the
/// block survive; only future sessions are suppressed.
#[tokio::test]
async fn blocked_key_drops_started_event() {
    let db = Database::open_memory().await.unwrap();
    let blocked_key = GameKey::steam("440");
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        blocklist_with([blocked_key.clone()]),
    );
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(manager.run(rx, shutdown_rx));

    tx.send(GameEvent::Started {
        key: blocked_key.clone(),
        display_name: "Team Fortress 2".into(),
        at: OffsetDateTime::now_utc(),
    })
    .await
    .unwrap();
    yield_for(50).await;

    // No application, no session: the event was swallowed.
    assert!(db
        .applications()
        .find_by_key(&blocked_key)
        .await
        .unwrap()
        .is_none());

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// Removing a key from the in-memory blocklist takes effect without
/// a daemon restart — the session manager reads the `Arc<RwLock>` on
/// every Started, so a write-side update (M6.6.3 will plug in the
/// D-Bus path here) is reflected on the very next event.
#[tokio::test]
async fn unblocking_a_key_lets_subsequent_sessions_through() {
    let db = Database::open_memory().await.unwrap();
    let key = GameKey::steam("440");
    let blocklist = blocklist_with([key.clone()]);
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        Arc::clone(&blocklist),
    );
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(manager.run(rx, shutdown_rx));

    // First Started: blocked.
    tx.send(GameEvent::Started {
        key: key.clone(),
        display_name: "Team Fortress 2".into(),
        at: OffsetDateTime::now_utc(),
    })
    .await
    .unwrap();
    yield_for(30).await;
    assert!(db.applications().find_by_key(&key).await.unwrap().is_none());

    // Unblock through the shared handle.
    blocklist.write().await.remove(&key);

    // Second Started: opens normally.
    tx.send(GameEvent::Started {
        key: key.clone(),
        display_name: "Team Fortress 2".into(),
        at: OffsetDateTime::now_utc(),
    })
    .await
    .unwrap();
    yield_for(50).await;
    assert!(db.applications().find_by_key(&key).await.unwrap().is_some());

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// A duplicate `Started` for an already-open key must not create a second
/// session; the manager logs a warning and drops the event.
#[tokio::test]
async fn duplicate_started_is_ignored() {
    let db = Database::open_memory().await.unwrap();
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        empty_blocklist(),
    );
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
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        empty_blocklist(),
    );
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
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        empty_blocklist(),
    );
    let closed = manager.recover_orphans().await.unwrap();
    assert_eq!(closed, 1);

    let refreshed = db.sessions().find_by_id(stale.id).await.unwrap().unwrap();
    assert_eq!(refreshed.exit_reason, Some(ExitReason::Recovered));
    assert_eq!(refreshed.ended_at, Some(refreshed.heartbeat_at));

    // Application aggregate stats must reflect the recovered session's
    // runtime — not doing so was the P1 data-loss bug this recovery
    // path used to have.
    let app_after = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(app_after.stat_run_count, 1);
    assert_eq!(app_after.stat_total_full, 600);
    assert_eq!(app_after.stat_total_interactive, 600);
    assert_eq!(app_after.stat_longest_full, 600);
    assert_eq!(app_after.last_played_at, Some(refreshed.heartbeat_at));
}

/// Regression guard for TIME-1: after a crash, systemd restarts the
/// daemon within `RestartSec` (5s), so the orphaned session's last
/// heartbeat is only seconds stale — far fresher than the old 2-minute
/// grace, which skipped it. Cold-start recovery must close *every* open
/// session regardless of heartbeat age: reaching this point means the
/// single-instance bus-name lock is held, so any open row is genuinely
/// from a dead prior process. Leaving it open let the partial-unique
/// index reject every future session for that app until the next manual
/// restart — silently dropping playtime.
#[tokio::test]
async fn recover_orphans_closes_recently_heartbeated_session() {
    let db = Database::open_memory().await.unwrap();

    let app = {
        use ludex_core::{GraphicsPlatform, Icons, NewApplication, ProcessArchitecture};
        db.applications()
            .create(NewApplication {
                launcher_type: ludex_core::LauncherType::Steam,
                launcher_id: "570".into(),
                product_name: "Dota 2".into(),
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
    let orphan = db
        .sessions()
        .begin(app.id, now - time::Duration::minutes(5))
        .await
        .unwrap();
    // Heartbeat only 10s ago — well inside the old 2-minute grace.
    db.sessions()
        .heartbeat(
            orphan.id,
            ludex_core::RuntimeSnapshot {
                full_runtime_seconds: 300,
                interactive_runtime_seconds: 300,
                at: now - time::Duration::seconds(10),
            },
        )
        .await
        .unwrap();

    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        empty_blocklist(),
    );
    let closed = manager.recover_orphans().await.unwrap();
    assert_eq!(
        closed, 1,
        "a fresh-heartbeat orphan at cold start must still be recovered",
    );

    let refreshed = db.sessions().find_by_id(orphan.id).await.unwrap().unwrap();
    assert_eq!(refreshed.exit_reason, Some(ExitReason::Recovered));
    assert!(refreshed.ended_at.is_some());
}

/// Idle time that accumulates during a session must be subtracted
/// from the session's interactive runtime when it closes.
///
/// This test fixes `idle_grace` at zero so every idle second is
/// billable — the cutscene-forgiveness behaviour gets its own
/// dedicated tests below.
#[tokio::test]
async fn idle_time_reduces_interactive_runtime() {
    let db = Database::open_memory().await.unwrap();
    let idle = Arc::new(IdleTracker::new());
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        Arc::clone(&idle),
        default_sleep_tracker(),
        config_with_idle_grace(Duration::ZERO),
        None,
        empty_blocklist(),
    );
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

    // User goes idle for 30s mid-session.
    idle.record_idle_interval(30);

    let end = at + time::Duration::seconds(120);
    tx.send(GameEvent::Stopped {
        key: GameKey::steam("440"),
        at: end,
    })
    .await
    .unwrap();
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
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.full_runtime_seconds, 120);
    // Interactive should be full − idle_during_session = 120 − 30 = 90.
    assert_eq!(session.interactive_runtime_seconds, 90);

    let refreshed = db.applications().find_by_id(app.id).await.unwrap().unwrap();
    assert_eq!(refreshed.stat_total_full, 120);
    assert_eq!(refreshed.stat_total_interactive, 90);

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// A short idle interval (cutscene-shaped) under the configured
/// grace must be fully credited as interactive — the user was
/// engaged, just not pressing keys. With grace = 60s and a 30s idle
/// interval, every interactive second of the session should equal
/// the full runtime.
#[tokio::test]
async fn idle_under_grace_is_fully_forgiven() {
    let db = Database::open_memory().await.unwrap();
    let idle = Arc::new(IdleTracker::new());
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        Arc::clone(&idle),
        default_sleep_tracker(),
        config_with_idle_grace(Duration::from_mins(1)),
        None,
        empty_blocklist(),
    );
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(manager.run(rx, shutdown_rx));
    let at = OffsetDateTime::now_utc();
    tx.send(GameEvent::Started {
        key: GameKey::steam("440"),
        display_name: "Cutscene Game".into(),
        at,
    })
    .await
    .unwrap();
    yield_for(50).await;

    // 30s "cutscene" — within the 60s grace.
    idle.record_idle_interval(30);

    let end = at + time::Duration::seconds(120);
    tx.send(GameEvent::Stopped {
        key: GameKey::steam("440"),
        at: end,
    })
    .await
    .unwrap();
    yield_for(50).await;

    let app = db
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .unwrap();
    let session = &db.sessions().list_for_application(app.id, 1).await.unwrap()[0];
    assert_eq!(session.full_runtime_seconds, 120);
    assert_eq!(
        session.interactive_runtime_seconds, 120,
        "30s idle under 60s grace should be fully forgiven",
    );

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// A long idle interval bills only the portion beyond the grace.
/// 600s idle under a 60s grace → 540s billable → interactive =
/// full − 540.
#[tokio::test]
async fn idle_above_grace_bills_only_the_tail() {
    let db = Database::open_memory().await.unwrap();
    let idle = Arc::new(IdleTracker::new());
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        Arc::clone(&idle),
        default_sleep_tracker(),
        config_with_idle_grace(Duration::from_mins(1)),
        None,
        empty_blocklist(),
    );
    let (tx, rx) = mpsc::channel::<GameEvent>(16);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = tokio::spawn(manager.run(rx, shutdown_rx));
    let at = OffsetDateTime::now_utc();
    tx.send(GameEvent::Started {
        key: GameKey::steam("440"),
        display_name: "AFK Game".into(),
        at,
    })
    .await
    .unwrap();
    yield_for(50).await;

    // 10-minute AFK during a 20-minute session.
    idle.record_idle_interval(10 * 60);

    let end = at + time::Duration::seconds(20 * 60);
    tx.send(GameEvent::Stopped {
        key: GameKey::steam("440"),
        at: end,
    })
    .await
    .unwrap();
    yield_for(50).await;

    let app = db
        .applications()
        .find_by_key(&GameKey::steam("440"))
        .await
        .unwrap()
        .unwrap();
    let session = &db.sessions().list_for_application(app.id, 1).await.unwrap()[0];
    assert_eq!(session.full_runtime_seconds, 20 * 60);
    // billable_idle = max(0, 600 - 60) = 540
    // interactive = full - billable = 1200 - 540 = 660
    assert_eq!(session.interactive_runtime_seconds, 660);

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// System suspend that happens during a session must be subtracted
/// from the session's *full* runtime (not just interactive). A game
/// that ran for 10 minutes, then the laptop slept for 8 hours, then
/// was closed should record 10 minutes, not 8 hours and 10 minutes.
#[tokio::test]
async fn suspended_time_reduces_full_runtime() {
    let db = Database::open_memory().await.unwrap();
    let sleep = Arc::new(SleepTracker::new());
    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        default_idle_tracker(),
        Arc::clone(&sleep),
        default_config(),
        None,
        empty_blocklist(),
    );
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

    // Simulate 8 hours of suspend mid-session.
    sleep.record_suspended_interval(8 * 3600);

    // Wall-clock end is 8h 10min after start.
    let end = at + time::Duration::seconds(8 * 3600 + 600);
    tx.send(GameEvent::Stopped {
        key: GameKey::steam("440"),
        at: end,
    })
    .await
    .unwrap();
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
    let session = &sessions[0];
    // full = wall (29400s) − suspend (28800s) = 600s = 10 minutes.
    assert_eq!(session.full_runtime_seconds, 600);
    // No idle; interactive equals full.
    assert_eq!(session.interactive_runtime_seconds, 600);

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
}

/// Only idle time that happened *during* the session is subtracted —
/// idle accumulated before `Started` must not count against the
/// session's interactive runtime.
#[tokio::test]
async fn pre_session_idle_does_not_count_against_session() {
    let db = Database::open_memory().await.unwrap();
    let idle = Arc::new(IdleTracker::new());
    // Before any session exists, pretend the user had already been
    // idle for two minutes while using the daemon to, say, browse the
    // GUI. The next session's interactive runtime must not be cut.
    idle.record_idle_interval(120);

    let manager = SessionManager::new(
        db.clone(),
        default_enrichment_ctx(),
        Arc::clone(&idle),
        default_sleep_tracker(),
        default_config(),
        None,
        empty_blocklist(),
    );
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

    let end = at + time::Duration::seconds(60);
    tx.send(GameEvent::Stopped {
        key: GameKey::steam("440"),
        at: end,
    })
    .await
    .unwrap();
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
    let session = &sessions[0];
    assert_eq!(session.full_runtime_seconds, 60);
    // No idle *during* the session → interactive equals full.
    assert_eq!(session.interactive_runtime_seconds, 60);

    shutdown_tx.send(true).unwrap();
    drop(tx);
    handle.await.unwrap().unwrap();
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
    let manager = SessionManager::new(
        db.clone(),
        ctx,
        default_idle_tracker(),
        default_sleep_tracker(),
        default_config(),
        None,
        empty_blocklist(),
    );
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
