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

/// Aggregate of every tunable daemon setting.
#[derive(Debug, Clone)]
pub struct TrackerConfig {
    /// Gate-layer knobs: GPU threshold, blocklist, launcher env vars.
    pub gate: GateConfig,
    /// Grace window between the tracked game losing foreground and
    /// a session actually closing. Read by the KWin source when
    /// arming its alt-tab grace timer.
    pub alt_tab_grace: Duration,
}

/// Shared handle to the tunable config. Clone the `Arc` — the
/// inner `RwLock` coordinates readers and writers.
pub type SharedConfig = Arc<RwLock<TrackerConfig>>;
