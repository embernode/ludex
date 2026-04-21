//! Top-level daemon wiring: open the database, spawn sources, spawn the
//! session manager, run until a shutdown signal arrives.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ludex_core::Database;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::session_manager::SessionManager;
use crate::sources::SteamSource;

const EVENT_CHANNEL_CAPACITY: usize = 128;

/// Initialise tracing from the `LUDEX_LOG` environment variable, defaulting
/// to `info` when unset.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_env("LUDEX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Locate the per-user database path at `$XDG_DATA_HOME/ludex/ludex.sqlite`,
/// falling back to `$HOME/.local/share/ludex/ludex.sqlite`.
fn default_database_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .context("neither XDG_DATA_HOME nor HOME is set")?;
    Ok(base.join("ludex").join("ludex.sqlite"))
}

/// Run the daemon until a termination signal is received.
pub async fn run() -> Result<()> {
    let db_path = default_database_path()?;
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create database dir {}", parent.display()))?;
    }
    info!(path = %db_path.display(), "opening database");
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("open database at {}", db_path.display()))?;

    let manager = SessionManager::new(db.clone());
    manager
        .recover_orphans()
        .await
        .context("cold-start orphan recovery")?;

    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Sources. Each source is optional: if its backing state is not
    // present, it logs and remains idle rather than failing the daemon.
    let mut source_handles = Vec::new();

    if let Some(steam) = SteamSource::detect_from_env() {
        let tx = event_tx.clone();
        let sd = shutdown_rx.clone();
        source_handles.push(tokio::spawn(async move {
            if let Err(e) = steam.run(tx, sd).await {
                warn!(error = %e, "Steam source exited with error");
            }
        }));
    } else {
        info!("Steam data directory not found; Steam source disabled");
    }

    // Drop our own sender so the event channel closes when all sources
    // exit.
    drop(event_tx);

    // Session manager runs inline on this task; source tasks run on
    // separate spawned tasks.
    let manager_handle = {
        let sd = shutdown_rx.clone();
        let manager = SessionManager::new(db.clone());
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
    manager_handle.await.context("session manager task")??;

    db.close().await;
    info!("shutdown complete");
    Ok(())
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
