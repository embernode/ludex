//! Top-level daemon wiring: open the database, spawn sources, spawn the
//! session manager, run until a shutdown signal arrives.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ludex_core::{default_database_path, Database};
use ludex_enrich::EnrichmentContext;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, watch, RwLock};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::{SharedConfig, TrackerConfig};
use crate::dbus::{self, TrackerNotification};
use crate::gate::{Gate, GateConfig};
use crate::idle::{self, IdleTracker};
use crate::session_manager::{SessionManager, SharedBlocklist};
use crate::sleep::{self, SleepTracker};
use crate::sources::{KWinForegroundSource, SteamSource};
use ludex_core::repo::{
    ALT_TAB_GRACE_SECONDS, DEFAULT_ALT_TAB_GRACE_SECONDS, DEFAULT_GPU_MEMORY_THRESHOLD_BYTES,
    GPU_MEMORY_THRESHOLD_BYTES,
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
    // Only log the sources whose enrichers are wired up today. Heroic
    // and Lutris paths are detected in the context for future use but
    // have no consumer yet; logging them advertised support we don't
    // actually provide.
    info!(
        desktop_dirs = enrichment_ctx.desktop_dirs.len(),
        steam = enrichment_ctx.steam_dir.is_some(),
        "enrichment context ready"
    );

    let idle_tracker = Arc::new(IdleTracker::new());
    let sleep_tracker = Arc::new(SleepTracker::new());

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

    // Public D-Bus API. Served on its own session-bus connection
    // (distinct from the KWin callback's org.kde.ludex.Tracker1)
    // so each service's lifecycle is independent.
    let shared_db = Arc::new(db.clone());
    let tracker_conn = dbus::serve(
        Arc::clone(&shared_db),
        Arc::clone(&blocklist),
        Arc::clone(&shared_config),
    )
    .await
    .context("register net.ludex.Tracker1 service")?;
    let (notif_tx, notif_rx) = mpsc::channel::<TrackerNotification>(NOTIFICATION_CHANNEL_CAPACITY);

    let manager = SessionManager::new(
        db.clone(),
        Arc::clone(&enrichment_ctx),
        Arc::clone(&idle_tracker),
        Arc::clone(&sleep_tracker),
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

    // Idle tracker watches logind on the system bus. Spawn before any
    // source so the baseline reflects the live state by the time the
    // first session opens.
    let idle_handle = {
        let tracker = Arc::clone(&idle_tracker);
        let sd = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = idle::run_watcher(tracker, sd).await {
                warn!(error = %e, "idle watcher exited with error");
            }
        })
    };

    // Sleep tracker polls the wall/monotonic clock drift every
    // `DEFAULT_TICK_SECONDS` and adds any detected suspend to the
    // accumulator. No D-Bus dependency; works across every desktop.
    let sleep_handle = {
        let tracker = Arc::clone(&sleep_tracker);
        let sd = shutdown_rx.clone();
        tokio::spawn(async move { sleep::run_watcher(tracker, sd).await })
    };

    // Periodic + on-shutdown database snapshots. Runs against the
    // same pool as everyone else; SQLite's VACUUM INTO is safe to
    // interleave with live writers.
    let backup_handle = {
        let db = db.clone();
        let sd = shutdown_rx.clone();
        tokio::spawn(async move { crate::backup::run_scheduler(db, sd).await })
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
    let _ = sleep_handle.await;
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
    let mut gate = GateConfig::default();
    match db
        .settings()
        .get_u64(
            GPU_MEMORY_THRESHOLD_BYTES,
            DEFAULT_GPU_MEMORY_THRESHOLD_BYTES,
        )
        .await
    {
        Ok(v) => gate.gpu_memory_threshold_bytes = v,
        Err(e) => warn!(
            error = %e,
            setting = GPU_MEMORY_THRESHOLD_BYTES,
            "settings read failed; using compiled-in default"
        ),
    }
    let alt_tab_grace = match db
        .settings()
        .get_u64(ALT_TAB_GRACE_SECONDS, DEFAULT_ALT_TAB_GRACE_SECONDS)
        .await
    {
        Ok(v) => Duration::from_secs(v),
        Err(e) => {
            warn!(
                error = %e,
                setting = ALT_TAB_GRACE_SECONDS,
                "settings read failed; using compiled-in default"
            );
            Duration::from_secs(DEFAULT_ALT_TAB_GRACE_SECONDS)
        }
    };
    info!(
        gpu_memory_threshold_bytes = gate.gpu_memory_threshold_bytes,
        alt_tab_grace_seconds = alt_tab_grace.as_secs(),
        "tracker configuration loaded"
    );
    TrackerConfig {
        gate,
        alt_tab_grace,
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
