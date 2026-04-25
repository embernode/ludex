//! Hot-reloadable daemon configuration.
//!
//! Settings the user can change at runtime (through the D-Bus API
//! the GUI drives) live behind `Arc<RwLock<TrackerConfig>>`. The
//! foreground source and the gate hold clones of the same handle, so
//! a value persisted from the GUI takes effect on the very next
//! decision or grace-timer arming — no daemon restart required.
//!
//! Values that do not change at runtime (the baseline blocklist, the
//! launcher environment-variable set) sit inside [`GateConfig`]
//! alongside the hot ones; the distinction is behavioural, not
//! structural. Moving one into "static" and one into "tunable"
//! would save a handful of allocations per decision at the cost of
//! a second type — not worth it while the setter path is used once
//! per user click.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::gate::GateConfig;

/// Tunables for the periodic database-backup scheduler. Lives on
/// [`TrackerConfig`] so the GUI can mutate it through the same
/// pathway as everything else; the scheduler subscribes to the
/// `backup_changed` `Notify` published by [`crate::daemon::run`] to
/// learn when this struct moved underneath it.
#[derive(Debug, Clone, Copy)]
pub struct BackupConfig {
    /// Wall-clock cadence between snapshots while the daemon runs.
    /// The scheduler clamps this above a minimum of one hour so a
    /// misconfigured value can't turn the timer into a hot loop.
    pub interval: Duration,
    /// Number of snapshots to keep after each successful backup.
    /// Older files are deleted in newest-first order. Pruning
    /// itself clamps zero to one — leaving nothing recoverable
    /// would be a surprising configuration outcome.
    pub retention: usize,
}

/// Aggregate of every tunable daemon setting.
#[derive(Debug, Clone)]
pub struct TrackerConfig {
    /// Gate-layer knobs: GPU threshold, blocklist, launcher env vars.
    pub gate: GateConfig,
    /// Grace window between the tracked game losing foreground and
    /// a session actually closing. Read by the KWin source when
    /// arming its alt-tab grace timer.
    pub alt_tab_grace: Duration,
    /// Whether losing focus should pause the session at all. When
    /// `false`, the foreground source ignores background windows
    /// entirely — sessions only end on process exit. When `true`
    /// (default), the grace window above applies.
    pub pause_when_backgrounded: bool,
    /// Per-idle-interval forgiveness window: the first `idle_grace`
    /// of every input-idle interval is credited as interactive
    /// rather than subtracted as AFK. Covers cutscenes, dialogue,
    /// long animations — read by the session manager when computing
    /// `interactive_runtime_seconds`.
    pub idle_grace: Duration,
    /// Periodic-backup scheduler tunables. Mutated by D-Bus setters;
    /// the scheduler is signalled separately to reset its timer.
    pub backup: BackupConfig,
}

/// Shared handle to the tunable config. Clone the `Arc` — the
/// inner `RwLock` coordinates readers and writers.
pub type SharedConfig = Arc<RwLock<TrackerConfig>>;
