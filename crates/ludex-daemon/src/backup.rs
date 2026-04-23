//! Periodic + on-shutdown database backup scheduler.
//!
//! Runs as a long-lived tokio task spawned by [`crate::daemon::run`].
//! Fires every `BACKUP_INTERVAL_HOURS` (default 24) while the
//! daemon is up, and once more when the shutdown signal flips.
//! After each successful snapshot the retained-count setting is
//! consulted and older files are pruned.
//!
//! Settings are read once at task start. The roadmap's follow-up
//! for live reload shares shape with the GPU-threshold reload: a
//! shared `Arc<RwLock<SchedulerConfig>>` the GUI can signal. Until
//! that lands the user restarts the daemon after changing the
//! interval or retention count.
//!
//! Snapshot creation is best-effort: any failure (no disk space,
//! permission denied, concurrent writer holding the WAL too long)
//! logs a warning and the scheduler continues rather than exiting.
//! The backup set stays consistent either way — the failed
//! snapshot simply isn't there.

use ludex_core::backup::snapshot_now;
use ludex_core::default_backup_dir;
use ludex_core::repo::{
    BACKUP_INTERVAL_HOURS, BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_INTERVAL_HOURS,
    DEFAULT_BACKUP_RETENTION_COUNT,
};
use ludex_core::Database;
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{info, instrument, warn};

/// Minimum backup cadence. Settings values below this are clamped
/// so a bad config can't turn the scheduler into a hot loop that
/// spams VACUUM INTO requests against the live pool.
const MIN_INTERVAL_SECONDS: u64 = 60 * 60;

/// Run the backup scheduler until `shutdown` fires, then take one
/// final snapshot. Never returns an error — backup failures are
/// logged but don't propagate up to the daemon, which has no good
/// response anyway.
#[instrument(name = "backup_scheduler", skip_all)]
pub async fn run_scheduler(db: Database, mut shutdown: watch::Receiver<bool>) {
    let (interval_secs, retention) = resolve_config(&db).await;
    let Some(dir) = default_backup_dir() else {
        warn!("neither XDG_DATA_HOME nor HOME is set; backup scheduler disabled");
        // Still await shutdown so the join handle resolves on exit.
        let _ = shutdown.changed().await;
        return;
    };
    info!(
        interval_seconds = interval_secs,
        retention,
        dir = %dir.display(),
        "backup scheduler started"
    );

    let mut tick = interval(Duration::from_secs(interval_secs));
    // Discard the immediate-fire first tick; backups should not
    // start firing the moment the daemon comes up.
    tick.tick().await;

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
            _ = tick.tick() => {
                run_once(&db, retention).await;
            }
        }
    }

    info!("taking final snapshot on shutdown");
    run_once(&db, retention).await;
    // `dir` kept in the info log above; discard to quieten the
    // unused-binding lint without reshaping the function.
    let _ = dir;
}

async fn resolve_config(db: &Database) -> (u64, usize) {
    let hours = db
        .settings()
        .get_u64(BACKUP_INTERVAL_HOURS, DEFAULT_BACKUP_INTERVAL_HOURS)
        .await
        .unwrap_or(DEFAULT_BACKUP_INTERVAL_HOURS);
    let retention = db
        .settings()
        .get_u64(BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_RETENTION_COUNT)
        .await
        .unwrap_or(DEFAULT_BACKUP_RETENTION_COUNT);
    let interval_secs = hours.saturating_mul(3_600).max(MIN_INTERVAL_SECONDS);
    (
        interval_secs,
        usize::try_from(retention).unwrap_or(usize::MAX),
    )
}

async fn run_once(db: &Database, retention: usize) {
    match snapshot_now(db, Some(retention)).await {
        Ok(path) => info!(path = %path.display(), "snapshot written"),
        Err(e) => warn!(error = %e, "snapshot failed"),
    }
}
