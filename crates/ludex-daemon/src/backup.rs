//! Periodic + on-shutdown database backup scheduler.
//!
//! Runs as a long-lived tokio task spawned by [`crate::daemon::run`].
//! Fires every [`BackupConfig::interval`] while the daemon is up, and
//! once more when the shutdown signal flips. After each successful
//! snapshot the retention count is consulted and older files are
//! pruned.
//!
//! Settings live-reload through [`SharedConfig`]: the D-Bus setters
//! mutate the shared config and call `notify_one` on the supplied
//! `Notify`; this task resets its [`tokio::time::Interval`] on each
//! such notification so a new cadence applies before the next tick
//! rather than after the in-flight one.
//!
//! Snapshot creation is best-effort: any failure (no disk space,
//! permission denied, concurrent writer holding the WAL too long)
//! logs a warning and the scheduler continues rather than exiting.
//! The backup set stays consistent either way — the failed
//! snapshot simply isn't there.
//!
//! [`BackupConfig::interval`]: crate::config::BackupConfig::interval
//! [`SharedConfig`]: crate::config::SharedConfig

use std::sync::Arc;

use ludex_core::backup::snapshot_now;
use ludex_core::default_backup_dir;
use ludex_core::Database;
use tokio::sync::{watch, Notify};
use tokio::time::{interval, Duration};
use tracing::{info, instrument, warn};

use crate::config::SharedConfig;

/// Minimum backup cadence. Settings values below this are clamped
/// so a bad config can't turn the scheduler into a hot loop that
/// spams VACUUM INTO requests against the live pool.
pub(crate) const MIN_INTERVAL_SECONDS: u64 = 60 * 60;

/// Run the backup scheduler until `shutdown` fires, then take one
/// final snapshot. Never returns an error — backup failures are
/// logged but don't propagate up to the daemon, which has no good
/// response anyway.
#[instrument(name = "backup_scheduler", skip_all)]
pub async fn run_scheduler(
    db: Database,
    config: SharedConfig,
    backup_changed: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    let Some(dir) = default_backup_dir() else {
        warn!("neither XDG_DATA_HOME nor HOME is set; backup scheduler disabled");
        // Still await shutdown so the join handle resolves on exit.
        let _ = shutdown.changed().await;
        return;
    };

    let mut current = read_clamped(&config).await;
    info!(
        interval_seconds = current.interval.as_secs(),
        retention = current.retention,
        dir = %dir.display(),
        "backup scheduler started"
    );
    let mut tick = make_tick(current.interval);

    loop {
        tokio::select! {
            // Notify-driven config reloads should win over a tick
            // that happens to fire in the same poll, so a freshly-
            // saved interval applies before the old one runs once
            // more.
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
            () = backup_changed.notified() => {
                let next = read_clamped(&config).await;
                if next.interval != current.interval {
                    info!(
                        old_interval_seconds = current.interval.as_secs(),
                        new_interval_seconds = next.interval.as_secs(),
                        "backup interval changed; resetting timer"
                    );
                    tick = make_tick(next.interval);
                }
                if next.retention != current.retention {
                    info!(
                        old_retention = current.retention,
                        new_retention = next.retention,
                        "backup retention changed",
                    );
                }
                current = next;
            }
            _ = tick.tick() => {
                run_once(&db, current.retention).await;
            }
        }
    }

    info!("taking final snapshot on shutdown");
    run_once(&db, current.retention).await;
    // Reference `dir` once outside the start-of-task log line so
    // the binding isn't dead.
    let _ = dir;
}

/// Snapshot of the live config the scheduler is using right now.
/// Captured by value so the loop body never holds the shared lock
/// across an await point.
#[derive(Clone, Copy)]
struct Current {
    interval: Duration,
    retention: usize,
}

/// Build a fresh interval timer that does *not* fire immediately.
/// `tokio::time::interval`'s default first tick resolves on the next
/// poll — undesired here because we just took a snapshot (or are
/// just starting) and want a full period before the next one.
fn make_tick(period: Duration) -> tokio::time::Interval {
    let mut t = interval(period);
    t.reset();
    t
}

/// Read the live config from `SharedConfig` and apply the
/// scheduler's safety floors. The interval clamp matches the value
/// surfaced by the D-Bus setter so the GUI sees the same number
/// after a write that the scheduler is actually using.
async fn read_clamped(config: &SharedConfig) -> Current {
    let cfg = config.read().await.backup;
    let min = Duration::from_secs(MIN_INTERVAL_SECONDS);
    let interval = cfg.interval.max(min);
    let retention = cfg.retention.max(1);
    Current {
        interval,
        retention,
    }
}

async fn run_once(db: &Database, retention: usize) {
    match snapshot_now(db, Some(retention)).await {
        Ok(path) => info!(path = %path.display(), "snapshot written"),
        Err(e) => warn!(error = %e, "snapshot failed"),
    }
}
