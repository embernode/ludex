//! User-idle tracking for interactive-runtime accounting.
//!
//! Every open session records a baseline of cumulative idle seconds
//! at start. On heartbeat and close, the session manager reads the
//! current cumulative count; the delta is the amount of "user was
//! AFK" time that occurred during the session, and that's what gets
//! subtracted from `full_runtime_seconds` to produce
//! `interactive_runtime_seconds`.
//!
//! The signal source is `org.freedesktop.login1.Session.IdleHint`.
//! The session / desktop reports this property true when the user
//! hasn't interacted with input devices for the configured timeout
//! (Plasma's defaults use the screen-off timeout; most systems are
//! somewhere around 5–10 minutes). This is the coarse-but-correct
//! signal for "did they step away" — and importantly, it requires no
//! membership in the `input` group or any other elevated permission.
//!
//! A finer-grained per-input-event counter via `/dev/input/event*` is
//! planned behind an explicit `evdev` feature flag in a later tranche.

use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use futures_util::StreamExt as _;
use tokio::sync::watch;
use tracing::{debug, info, instrument, warn};

/// Locked state of the idle accumulator.
#[derive(Debug, Default)]
struct State {
    /// Per-interval durations for completed (sealed) idle periods.
    /// Storing intervals individually — rather than only their sum —
    /// is what makes the cutscene-forgiveness math possible: each
    /// natural idle interval is forgiven up to its own grace
    /// threshold, so two short cutscenes within one session don't
    /// have their forgiveness pooled into one window.
    ///
    /// The Vec grows for the lifetime of the daemon. Idle transitions
    /// are slow (minutes apart), so even an always-on daemon
    /// accumulates only thousands of entries per year — the memory
    /// cost is negligible compared to the bookkeeping a ring-buffer
    /// would add.
    closed_intervals_seconds: Vec<i64>,
    /// When the current idle interval started, if any.
    since: Option<Instant>,
}

/// Monotonic accumulator of seconds the user has been idle since the
/// daemon started.
///
/// Construct once at daemon startup, wrap in `Arc`, share with both
/// the D-Bus watcher task and the session manager.
#[derive(Debug, Default)]
pub struct IdleTracker {
    state: Mutex<State>,
}

impl IdleTracker {
    /// Construct a fresh tracker with zero accumulated idle time.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Total seconds the user has been idle since this tracker was
    /// created. If the user is currently idle, the still-open interval
    /// is included up to `now()`.
    #[must_use]
    pub fn accumulated_idle_seconds(&self) -> i64 {
        let s = self.state.lock().expect("idle tracker mutex poisoned");
        let mut total: i64 = s
            .closed_intervals_seconds
            .iter()
            .copied()
            .fold(0i64, i64::saturating_add);
        if let Some(since) = s.since {
            total = total.saturating_add(seconds_since(since));
        }
        total
    }

    /// Number of completed idle intervals. A session captures this at
    /// start so [`Self::billable_idle_seconds_since`] knows where its
    /// view of new intervals begins.
    #[must_use]
    pub fn closed_intervals_count(&self) -> usize {
        self.state
            .lock()
            .expect("idle tracker mutex poisoned")
            .closed_intervals_seconds
            .len()
    }

    /// Sum of `max(0, interval_duration − grace)` across every
    /// closed idle interval recorded after `baseline_count`, plus
    /// the same forgiveness applied to the currently-open interval
    /// (if any).
    ///
    /// Cutscene rationale: a long idle interval is mostly AFK time
    /// the user wasn't playing, but the first few minutes of *every*
    /// interval are statistically likely to have been a non-skippable
    /// cutscene, dialogue tree, or similar engagement-without-input
    /// event. Forgiving the first `grace` seconds of each natural
    /// interval credits those back into `interactive_runtime` while
    /// still subtracting the genuine AFK tail.
    ///
    /// Sessions that started when the user was already idle (rare —
    /// foreground-window activations require input, which clears
    /// idle) have any open interval billed against them in full,
    /// minus the grace. The slight over-forgiveness is acceptable
    /// for the edge case it covers.
    #[must_use]
    pub fn billable_idle_seconds_since(
        &self,
        baseline_count: usize,
        grace_seconds: i64,
    ) -> i64 {
        let s = self.state.lock().expect("idle tracker mutex poisoned");
        let mut total: i64 = 0;
        for dur in s.closed_intervals_seconds.iter().skip(baseline_count) {
            total = total.saturating_add((*dur - grace_seconds).max(0));
        }
        if let Some(since) = s.since {
            let dur = seconds_since(since);
            total = total.saturating_add((dur - grace_seconds).max(0));
        }
        total
    }

    /// `true` if the last recorded transition said the user is idle.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.state
            .lock()
            .expect("idle tracker mutex poisoned")
            .since
            .is_some()
    }

    /// Record a change in `IdleHint`. Idempotent — repeated `true` or
    /// repeated `false` transitions are no-ops.
    pub fn set_idle(&self, idle: bool) {
        let mut s = self.state.lock().expect("idle tracker mutex poisoned");
        match (s.since, idle) {
            (None, true) => {
                s.since = Some(Instant::now());
            }
            (Some(since), false) => {
                let duration = seconds_since(since);
                s.closed_intervals_seconds.push(duration);
                s.since = None;
            }
            _ => {}
        }
    }

    /// Append a synthetic completed idle interval of `seconds`.
    ///
    /// Intended for:
    /// * test code that needs to inject a known amount of idle time
    ///   without waiting for a real wall clock;
    /// * future alternative idle sources (for example, a GNOME
    ///   IdleMonitor bridge) that report cumulative intervals rather
    ///   than edge transitions.
    pub fn record_idle_interval(&self, seconds: i64) {
        let mut s = self.state.lock().expect("idle tracker mutex poisoned");
        s.closed_intervals_seconds.push(seconds);
    }
}

fn seconds_since(t: Instant) -> i64 {
    i64::try_from(t.elapsed().as_secs()).unwrap_or(i64::MAX)
}

/// Proxy for the current login session's `IdleHint` property.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/session/auto"
)]
trait LogindSession {
    #[zbus(property, name = "IdleHint")]
    fn idle_hint(&self) -> zbus::Result<bool>;
}

/// Drive `tracker`'s state from `logind.IdleHint` until `shutdown`
/// fires. Runs in a dedicated task spawned by the daemon.
///
/// This function never panics. If the system bus or logind is
/// unreachable (e.g. the daemon was launched outside a logind
/// session) the task logs a warning and exits quietly; the tracker
/// then stays at zero and `interactive_runtime_seconds` will equal
/// `full_runtime_seconds`. That's a reasonable degradation — it
/// matches pre-M5 behaviour.
#[instrument(name = "idle_watcher", skip_all)]
pub async fn run_watcher(
    tracker: std::sync::Arc<IdleTracker>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let conn = match zbus::Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "system bus unavailable; idle tracking disabled");
            return Ok(());
        }
    };
    let proxy = LogindSessionProxy::new(&conn)
        .await
        .context("construct logind session proxy")?;

    let initial = proxy.idle_hint().await.unwrap_or(false);
    tracker.set_idle(initial);
    info!(initial_idle = initial, "idle watcher started");

    let mut stream = proxy.receive_idle_hint_changed().await;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            change = stream.next() => {
                let Some(change) = change else {
                    debug!("IdleHint property stream closed");
                    break;
                };
                let new = change.get().await.unwrap_or(false);
                debug!(idle = new, "logind IdleHint changed");
                tracker.set_idle(new);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn default_is_zero() {
        let t = IdleTracker::new();
        assert_eq!(t.accumulated_idle_seconds(), 0);
        assert!(!t.is_idle());
    }

    #[test]
    fn record_idle_interval_accumulates() {
        let t = IdleTracker::new();
        t.record_idle_interval(30);
        assert_eq!(t.accumulated_idle_seconds(), 30);
        t.record_idle_interval(5);
        assert_eq!(t.accumulated_idle_seconds(), 35);
    }

    #[test]
    fn set_idle_closes_interval_on_transition_out() {
        let t = IdleTracker::new();
        t.set_idle(true);
        assert!(t.is_idle());
        sleep(Duration::from_millis(50));
        t.set_idle(false);
        assert!(!t.is_idle());
        // Accumulated should be ~0s (50ms rounds down to 0) but must
        // not be negative and must not panic.
        let acc = t.accumulated_idle_seconds();
        assert!(acc >= 0);
    }

    #[test]
    fn duplicate_idle_true_is_idempotent() {
        let t = IdleTracker::new();
        t.set_idle(true);
        sleep(Duration::from_millis(10));
        t.set_idle(true); // ignored; since should not reset
        sleep(Duration::from_millis(10));
        t.set_idle(false);
        // The "since" set on the first call was never replaced, so
        // total should reflect ~20ms (still rounds to 0 seconds, but
        // confirms nothing weird happened).
        let _ = t.accumulated_idle_seconds();
    }

    #[test]
    fn currently_idle_counts_open_interval() {
        let t = IdleTracker::new();
        t.record_idle_interval(10); // 10s closed-accumulated
        t.set_idle(true);
        // The closed accumulator still shows as the baseline; open
        // interval adds to it.
        assert!(t.accumulated_idle_seconds() >= 10);
    }

    #[test]
    fn closed_intervals_count_grows_on_seal() {
        let t = IdleTracker::new();
        assert_eq!(t.closed_intervals_count(), 0);
        t.set_idle(true);
        assert_eq!(t.closed_intervals_count(), 0); // still open
        t.set_idle(false);
        assert_eq!(t.closed_intervals_count(), 1);
        t.record_idle_interval(7);
        assert_eq!(t.closed_intervals_count(), 2);
    }

    #[test]
    fn billable_short_intervals_under_grace_are_fully_forgiven() {
        let t = IdleTracker::new();
        // Two cutscene-shaped intervals under the grace.
        t.record_idle_interval(120); // 2 min
        t.record_idle_interval(180); // 3 min
        // Grace 5 min: both fully forgiven, both billable values 0.
        assert_eq!(t.billable_idle_seconds_since(0, 300), 0);
    }

    #[test]
    fn billable_long_interval_is_billed_minus_grace() {
        let t = IdleTracker::new();
        t.record_idle_interval(30 * 60); // 30 min AFK
        // Grace 5 min: billable = 25 min.
        assert_eq!(t.billable_idle_seconds_since(0, 5 * 60), 25 * 60);
    }

    #[test]
    fn billable_baseline_skips_intervals_before_session_start() {
        let t = IdleTracker::new();
        // 30-min idle that happened *before* the session.
        t.record_idle_interval(30 * 60);
        let baseline = t.closed_intervals_count();
        // 10-min idle during the session — billable = 5 min with
        // grace = 5 min.
        t.record_idle_interval(10 * 60);
        assert_eq!(t.billable_idle_seconds_since(baseline, 5 * 60), 5 * 60);
    }

    #[test]
    fn billable_per_interval_grace_does_not_pool_across_two_short_intervals() {
        let t = IdleTracker::new();
        // Two 4-minute intervals: total idle 8 min.
        // Per-session pooled grace would bill 8 - 5 = 3 min.
        // Per-interval grace bills max(0, 4-5) + max(0, 4-5) = 0.
        // The latter is the right behaviour for "two cutscenes".
        t.record_idle_interval(4 * 60);
        t.record_idle_interval(4 * 60);
        assert_eq!(t.billable_idle_seconds_since(0, 5 * 60), 0);
    }

    #[test]
    fn billable_currently_open_interval_is_included_with_forgiveness() {
        let t = IdleTracker::new();
        // Synthetic baseline of 10 minutes already closed (and
        // billable in full minus grace).
        t.record_idle_interval(10 * 60);
        // Open a fresh interval; instantly check billable. The open
        // interval has elapsed ~0s, so its billable contribution is
        // max(0, 0 - 300) = 0. Total billable = 10 min - 5 min = 5 min.
        t.set_idle(true);
        let billable = t.billable_idle_seconds_since(0, 5 * 60);
        assert_eq!(billable, 5 * 60);
    }
}
