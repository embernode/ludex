//! System-suspend tracking for runtime accounting.
//!
//! The session manager subtracts suspended seconds from a session's
//! `full_runtime_seconds` so that an eight-hour laptop-closed
//! stretch doesn't count as gameplay. This module provides the
//! accumulator; see `session_manager.rs` for how it's consumed.
//!
//! # Why clock drift, not `PrepareForSleep`?
//!
//! The obvious implementation is to subscribe to
//! `org.freedesktop.login1.Manager.PrepareForSleep` on the system
//! bus. That signal fires twice per suspend cycle — once before the
//! system freezes, once on resume — and in principle lets us record
//! the boundary timestamps.
//!
//! In practice the pre-suspend signal arrives *just* before the
//! kernel freeze, and there is no guarantee the daemon is scheduled
//! before the freeze kicks in. Missing the pre-suspend half means
//! missing the entire interval for that cycle.
//!
//! A clock-drift detector is simpler and more reliable. On each
//! tick we compare the wall-clock delta against the monotonic
//! delta. Under normal operation the two advance in lockstep and
//! the delta is zero. During suspend, the wall clock advances (it
//! reflects real time) while `std::time::Instant` does not (it
//! uses `CLOCK_MONOTONIC`, which is paused across suspend on Linux
//! by default). The difference is the suspend duration — regardless
//! of how the scheduler treated us before the freeze.
//!
//! A threshold of 5 seconds prevents tiny scheduling latencies from
//! being misrecorded as suspends.

use std::sync::Mutex;
use std::time::Instant;

use time::OffsetDateTime;
use tokio::sync::watch;
use tracing::{debug, info, instrument};

/// A tick below this many seconds of wall/mono drift is discarded as
/// ordinary scheduling latency rather than a suspend event.
const MIN_SUSPEND_THRESHOLD_SECONDS: i64 = 5;

/// Default polling cadence of the watcher task. The upper bound on
/// latency between the real wake-up and the suspend being recorded.
pub const DEFAULT_TICK_SECONDS: u64 = 10;

#[derive(Debug)]
struct State {
    total_suspended_seconds: i64,
    last_wall: OffsetDateTime,
    last_mono: Instant,
}

/// Monotonic accumulator of seconds the system has spent suspended
/// since the daemon started. Shared between the polling watcher task
/// and the session manager via `Arc`.
#[derive(Debug)]
pub struct SleepTracker {
    state: Mutex<State>,
}

impl Default for SleepTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SleepTracker {
    /// Construct a fresh tracker anchored at the current instant.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                total_suspended_seconds: 0,
                last_wall: OffsetDateTime::now_utc(),
                last_mono: Instant::now(),
            }),
        }
    }

    /// Total seconds the system has spent suspended since the tracker
    /// was constructed.
    #[must_use]
    pub fn accumulated_suspended_seconds(&self) -> i64 {
        self.state
            .lock()
            .expect("sleep tracker mutex poisoned")
            .total_suspended_seconds
    }

    /// Poll the wall vs. monotonic deltas. Any wall-clock jump beyond
    /// [`MIN_SUSPEND_THRESHOLD_SECONDS`] above the monotonic-clock
    /// advance is recorded as a suspend. Returns the number of
    /// suspended seconds this tick detected (zero on normal ticks).
    pub fn tick(&self) -> i64 {
        let mut s = self.state.lock().expect("sleep tracker mutex poisoned");
        let now_wall = OffsetDateTime::now_utc();
        let now_mono = Instant::now();

        let wall_delta = (now_wall - s.last_wall).whole_seconds().max(0);
        let mono_delta =
            i64::try_from(now_mono.duration_since(s.last_mono).as_secs()).unwrap_or(i64::MAX);

        let drift = wall_delta.saturating_sub(mono_delta).max(0);
        let accepted = if drift >= MIN_SUSPEND_THRESHOLD_SECONDS {
            drift
        } else {
            0
        };
        s.total_suspended_seconds = s.total_suspended_seconds.saturating_add(accepted);
        s.last_wall = now_wall;
        s.last_mono = now_mono;
        accepted
    }

    /// Add `seconds` to the accumulator.
    ///
    /// Intended for test code that needs to inject a known suspend
    /// without manipulating the wall clock, and for future
    /// alternative suspend sources (for example, a system-bus
    /// subscription that deduplicates with the clock-drift detector).
    pub fn record_suspended_interval(&self, seconds: i64) {
        let mut s = self.state.lock().expect("sleep tracker mutex poisoned");
        s.total_suspended_seconds = s.total_suspended_seconds.saturating_add(seconds);
    }
}

/// Poll [`SleepTracker::tick`] every [`DEFAULT_TICK_SECONDS`] seconds
/// until `shutdown` fires.
#[instrument(name = "sleep_watcher", skip_all)]
pub async fn run_watcher(
    tracker: std::sync::Arc<SleepTracker>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(DEFAULT_TICK_SECONDS));
    // Discard the immediate first tick so we don't bill the startup
    // latency as a suspend.
    interval.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = interval.tick() => {
                let detected = tracker.tick();
                if detected > 0 {
                    info!(seconds = detected, "detected system suspend via clock drift");
                } else {
                    debug!("sleep tick — no suspend detected");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zero() {
        let t = SleepTracker::new();
        assert_eq!(t.accumulated_suspended_seconds(), 0);
    }

    #[test]
    fn record_suspended_interval_accumulates() {
        let t = SleepTracker::new();
        t.record_suspended_interval(60);
        t.record_suspended_interval(120);
        assert_eq!(t.accumulated_suspended_seconds(), 180);
    }

    #[test]
    fn tick_under_normal_operation_is_zero() {
        let t = SleepTracker::new();
        // No actual suspend occurs between construction and tick,
        // so the drift must be well below the 5s threshold.
        assert_eq!(t.tick(), 0);
        assert_eq!(t.accumulated_suspended_seconds(), 0);
    }

    #[test]
    fn tick_does_not_go_negative() {
        let t = SleepTracker::new();
        for _ in 0..10 {
            t.tick();
        }
        assert!(t.accumulated_suspended_seconds() >= 0);
    }
}
