//! Top-level daemon wiring: open the database, spawn sources, spawn the
//! session manager, run until a shutdown signal arrives.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ludex_core::{default_database_path, Database};
use ludex_enrich::EnrichmentContext;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, watch, Notify, RwLock};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::{BackupConfig, SharedConfig, TrackerConfig};
use crate::dbus::{self, TrackerNotification};
use crate::gate::{Gate, GateConfig};
use crate::idle::IdleTracker;
use crate::idle_wayland;
use crate::session_manager::{SessionManager, SharedBlocklist, SystemClock};
use crate::sources::{KWinForegroundSource, SteamSource};
use ludex_core::repo::{
    ALT_TAB_GRACE_SECONDS, BACKUP_INTERVAL_HOURS, BACKUP_RETENTION_COUNT,
    DEFAULT_ALT_TAB_GRACE_SECONDS, DEFAULT_BACKUP_INTERVAL_HOURS, DEFAULT_BACKUP_RETENTION_COUNT,
    DEFAULT_GPU_MEMORY_THRESHOLD_BYTES, DEFAULT_IDLE_GRACE_SECONDS,
    DEFAULT_PAUSE_WHEN_BACKGROUNDED, GPU_MEMORY_THRESHOLD_BYTES, IDLE_GRACE_SECONDS,
    PAUSE_WHEN_BACKGROUNDED,
};

const EVENT_CHANNEL_CAPACITY: usize = 128;
const NOTIFICATION_CHANNEL_CAPACITY: usize = 64;

/// Initialise tracing from the `LUDEX_LOG` environment variable, defaulting
/// to `info` when unset.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_env("LUDEX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Run the daemon until a termination signal is received.
#[allow(
    clippy::too_many_lines,
    reason = "linear startup wiring; splitting it would obscure the boot order"
)]
pub async fn run() -> Result<()> {
    let db_path = default_database_path().context("neither XDG_DATA_HOME nor HOME is set")?;
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create database dir {}", parent.display()))?;
    }
    info!(path = %db_path.display(), "opening database");
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("open database at {}", db_path.display()))?;

    // Resolve the tunable tracker configuration from persisted
    // settings, falling back to compiled-in defaults when a row is
    // absent. The shared handle below is cloned into every consumer
    // (gate, foreground source, D-Bus setter) so a GUI-driven
    // update takes effect without a daemon restart.
    let shared_config: SharedConfig = Arc::new(RwLock::new(resolve_tracker_config(&db).await));

    let enrichment_ctx = Arc::new(EnrichmentContext::detect_from_env());
    info!(
        desktop_dirs = enrichment_ctx.desktop_dirs.len(),
        steam = enrichment_ctx.steam_dir.is_some(),
        lutris = enrichment_ctx.lutris_pga_db.is_some(),
        heroic = enrichment_ctx.heroic_config_dir.is_some(),
        "enrichment context ready"
    );

    let idle_tracker = Arc::new(IdleTracker::new());

    // Hydrate the in-memory blocklist from the DB. Future D-Bus
    // write methods will mutate the same Arc<RwLock<…>> so the
    // session manager sees additions and removals immediately
    // without a reload signal.
    let blocklist: SharedBlocklist =
        Arc::new(RwLock::new(db.blocked().list().await.unwrap_or_else(|e| {
            warn!(error = %e, "could not load blocklist; treating as empty");
            HashSet::new()
        })));
    info!(blocked = blocklist.read().await.len(), "blocklist loaded");

    // Signal channel from the D-Bus backup setters to the scheduler.
    // The setter writes the new value to `shared_config`, then
    // `notify_one`s; the scheduler wakes, re-reads, and resets its
    // timer if the cadence moved. Cheap to keep around even when
    // nothing is listening — `Notify` accumulates at most one permit.
    let backup_changed = Arc::new(Notify::new());

    // Public D-Bus API. Served on its own session-bus connection
    // (distinct from the KWin callback's org.kde.ludex.Tracker1)
    // so each service's lifecycle is independent.
    let shared_db = Arc::new(db.clone());
    let tracker_conn = dbus::serve(
        Arc::clone(&shared_db),
        Arc::clone(&blocklist),
        Arc::clone(&shared_config),
        Arc::clone(&backup_changed),
    )
    .await
    .context("register net.ludex.Tracker1 service")?;
    let (notif_tx, notif_rx) = mpsc::channel::<TrackerNotification>(NOTIFICATION_CHANNEL_CAPACITY);

    let manager = SessionManager::new(
        db.clone(),
        Arc::clone(&enrichment_ctx),
        Arc::clone(&idle_tracker),
        Arc::new(SystemClock),
        Arc::clone(&shared_config),
        Some(notif_tx),
        Arc::clone(&blocklist),
    );
    manager
        .recover_orphans()
        .await
        .context("cold-start orphan recovery")?;

    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Drive the D-Bus notifier from the same shutdown channel as
    // every other background task.
    let notifier_handle = {
        let conn = tracker_conn.clone();
        let sd = shutdown_rx.clone();
        tokio::spawn(async move { dbus::run_notifier(conn, notif_rx, sd).await })
    };

    // Idle source: ext-idle-notify-v1 on the Wayland session, falling
    // back to logind IdleHint where no usable Wayland session exists.
    // Spawn before any game source so the baseline reflects the live
    // state by the time the first session opens.
    let idle_handle = {
        let tracker = Arc::clone(&idle_tracker);
        let sd = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = idle_wayland::run_watcher(tracker, sd).await {
                warn!(error = %e, "idle watcher exited with error");
            }
        })
    };

    // Periodic + on-shutdown database snapshots. Runs against the
    // same pool as everyone else; SQLite's VACUUM INTO is safe to
    // interleave with live writers. The shared config + Notify let
    // GUI-driven changes to interval and retention apply without a
    // daemon restart.
    let backup_handle = {
        let db = db.clone();
        let cfg = Arc::clone(&shared_config);
        let notify = Arc::clone(&backup_changed);
        let sd = shutdown_rx.clone();
        tokio::spawn(async move { crate::backup::run_scheduler(db, cfg, notify, sd).await })
    };

    let source_handles =
        spawn_sources(event_tx, shutdown_rx.clone(), Arc::clone(&shared_config)).await;

    // Session manager runs on its own task so the main task stays free
    // to handle the shutdown signal.
    let manager_handle = {
        let sd = shutdown_rx.clone();
        tokio::spawn(manager.run(event_rx, sd))
    };

    // Wait for a termination signal.
    wait_for_shutdown().await;
    info!("termination signal received, shutting down");
    let _ = shutdown_tx.send(true);

    // Give spawned tasks a moment to shut down cleanly.
    for h in source_handles {
        let _ = h.await;
    }
    let _ = idle_handle.await;
    // Back up BEFORE closing the database pool — run_scheduler's
    // shutdown path needs a live pool to take the final snapshot.
    let _ = backup_handle.await;
    let _ = notifier_handle.await;
    manager_handle.await.context("session manager task")??;
    drop(tracker_conn);

    db.close().await;
    info!("shutdown complete");
    Ok(())
}

/// Spawn every available event source on its own tokio task,
/// cloning `event_tx` into each. Sources whose backing state is
/// missing log once and remain idle rather than failing the daemon.
/// Dropping `event_tx` at the end lets the event channel close
/// naturally when every source exits.
async fn spawn_sources(
    event_tx: mpsc::Sender<crate::event::GameEvent>,
    shutdown_rx: watch::Receiver<bool>,
    shared_config: SharedConfig,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();

    if let Some(steam) = SteamSource::detect_from_env() {
        let tx = event_tx.clone();
        let sd = shutdown_rx.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = steam.run(tx, sd).await {
                warn!(error = %e, "Steam source exited with error");
            }
        }));
    } else {
        info!("Steam data directory not found; Steam source disabled");
    }

    if KWinForegroundSource::is_kwin_available().await {
        let gate = Gate::new(Arc::clone(&shared_config));
        let kwin = KWinForegroundSource::new(gate, shared_config);
        let tx = event_tx.clone();
        let sd = shutdown_rx.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = kwin.install_and_run(tx, sd).await {
                warn!(error = %e, "KWin foreground source exited with error");
            }
        }));
    } else {
        info!("org.kde.KWin not present on session bus; foreground source disabled");
    }

    // Drop our clone of the sender so the event channel closes
    // when all sources exit.
    drop(event_tx);
    handles
}

/// Assemble a [`TrackerConfig`] from persisted settings, falling
/// back to compiled-in defaults when a row is absent or a read
/// errors. A transient DB read error must never stop the daemon
/// from starting — log it and use the default.
async fn resolve_tracker_config(db: &Database) -> TrackerConfig {
    let gpu_memory_threshold_bytes = load_u64(
        db,
        GPU_MEMORY_THRESHOLD_BYTES,
        DEFAULT_GPU_MEMORY_THRESHOLD_BYTES,
    )
    .await;
    let alt_tab_grace_seconds =
        load_u64(db, ALT_TAB_GRACE_SECONDS, DEFAULT_ALT_TAB_GRACE_SECONDS).await;
    let pause_when_backgrounded =
        load_bool(db, PAUSE_WHEN_BACKGROUNDED, DEFAULT_PAUSE_WHEN_BACKGROUNDED).await;
    let idle_grace_seconds = load_u64(db, IDLE_GRACE_SECONDS, DEFAULT_IDLE_GRACE_SECONDS).await;
    let backup_interval_hours =
        load_u64(db, BACKUP_INTERVAL_HOURS, DEFAULT_BACKUP_INTERVAL_HOURS).await;
    let backup_retention =
        load_u64(db, BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_RETENTION_COUNT).await;

    let gate = GateConfig {
        gpu_memory_threshold_bytes,
        ..GateConfig::default()
    };
    let alt_tab_grace = Duration::from_secs(alt_tab_grace_seconds);
    let idle_grace = Duration::from_secs(idle_grace_seconds);
    let backup = BackupConfig {
        interval: Duration::from_secs(backup_interval_hours.saturating_mul(3_600)),
        retention: usize::try_from(backup_retention).unwrap_or(usize::MAX),
    };

    info!(
        gpu_memory_threshold_bytes,
        alt_tab_grace_seconds,
        pause_when_backgrounded,
        idle_grace_seconds,
        backup_interval_hours,
        backup_retention,
        "tracker configuration loaded"
    );
    TrackerConfig {
        gate,
        alt_tab_grace,
        pause_when_backgrounded,
        idle_grace,
        backup,
    }
}

/// Read a `u64` setting, returning `fallback` (with a warn-log) on
/// any read error. Used by `resolve_tracker_config` to keep the
/// reload path uniform across every tunable.
async fn load_u64(db: &Database, key: &str, fallback: u64) -> u64 {
    match db.settings().get_u64(key, fallback).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, setting = key, "settings read failed; using compiled-in default");
            fallback
        }
    }
}

/// Read a `bool` setting with the same fallback semantics as
/// [`load_u64`].
async fn load_bool(db: &Database, key: &str, fallback: bool) -> bool {
    match db.settings().get_bool(key, fallback).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, setting = key, "settings read failed; using compiled-in default");
            fallback
        }
    }
}

async fn wait_for_shutdown() {
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "SIGTERM handler unavailable; Ctrl-C only");
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = sigterm.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
