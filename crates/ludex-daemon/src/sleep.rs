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
//! tick we compare the `CLOCK_BOOTTIME` delta against the
//! `CLOCK_MONOTONIC` delta. Under normal operation the two advance
//! in lockstep. During suspend, `CLOCK_BOOTTIME` keeps counting
//! while `CLOCK_MONOTONIC` (what `std::time::Instant` reads on
//! Linux) is paused. The difference is exactly the suspend duration
//! — regardless of how the scheduler treated us before the freeze.
//!
//! # Why `CLOCK_BOOTTIME`, not the wall clock?
//!
//! An earlier revision compared the wall clock against
//! `CLOCK_MONOTONIC`. That works for suspend, but the wall clock
//! also moves when NTP steps it: a forward correction during a
//! session read exactly like a suspend and was silently deducted
//! from recorded playtime. `CLOCK_BOOTTIME` counts suspended time
//! yet is immune to clock adjustments, so the boottime-vs-monotonic
//! drift isolates suspend and nothing else.
//!
//! A threshold of 5 seconds prevents tiny read-ordering skew between
//! the two clock fetches from being misrecorded as suspends.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustix::time::{clock_gettime, ClockId};
use tokio::sync::watch;
use tracing::{debug, info, instrument};

/// A tick below this many seconds of boottime/monotonic drift is
/// discarded as clock-read skew rather than a suspend event.
const MIN_SUSPEND_THRESHOLD_SECONDS: i64 = 5;

/// Default polling cadence of the watcher task. The upper bound on
/// latency between the real wake-up and the suspend being recorded.
pub const DEFAULT_TICK_SECONDS: u64 = 10;

/// Current `CLOCK_BOOTTIME` reading as a [`Duration`] since boot.
fn boottime_now() -> Duration {
    let ts = clock_gettime(ClockId::Boottime);
    Duration::new(
        u64::try_from(ts.tv_sec).unwrap_or(0),
        u32::try_from(ts.tv_nsec).unwrap_or(0),
    )
}

#[derive(Debug)]
struct State {
    total_suspended_seconds: i64,
    last_boot: Duration,
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
        Self::anchored(boottime_now(), Instant::now())
    }

    /// Construct a tracker anchored at explicit clock readings.
    /// Production goes through [`Self::new`]; tests use this to make
    /// [`Self::observe`] fully deterministic.
    fn anchored(boot: Duration, mono: Instant) -> Self {
        Self {
            state: Mutex::new(State {
                total_suspended_seconds: 0,
                last_boot: boot,
                last_mono: mono,
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

    /// Poll the boottime vs. monotonic deltas. Any boottime advance
    /// beyond [`MIN_SUSPEND_THRESHOLD_SECONDS`] above the
    /// monotonic-clock advance is recorded as a suspend. Returns the
    /// number of suspended seconds this tick detected (zero on
    /// normal ticks).
    pub fn tick(&self) -> i64 {
        self.observe(boottime_now(), Instant::now())
    }

    /// [`Self::tick`] with the clock readings injected. Split out so
    /// tests can drive the detector with fabricated suspends.
    fn observe(&self, now_boot: Duration, now_mono: Instant) -> i64 {
        let mut s = self.state.lock().expect("sleep tracker mutex poisoned");

        let boot_delta =
            i64::try_from(now_boot.saturating_sub(s.last_boot).as_secs()).unwrap_or(i64::MAX);
        let mono_delta = i64::try_from(now_mono.saturating_duration_since(s.last_mono).as_secs())
            .unwrap_or(i64::MAX);

        let drift = boot_delta.saturating_sub(mono_delta).max(0);
        let accepted = if drift >= MIN_SUSPEND_THRESHOLD_SECONDS {
            drift
        } else {
            0
        };
        s.total_suspended_seconds = s.total_suspended_seconds.saturating_add(accepted);
        s.last_boot = now_boot;
        s.last_mono = now_mono;
        accepted
    }

    /// Add `seconds` to the accumulator.
    ///
    /// Intended for test code that needs to inject a known suspend
    /// without manipulating the clocks, and for future alternative
    /// suspend sources (for example, a system-bus subscription that
    /// deduplicates with the clock-drift detector).
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

    const SEC: Duration = Duration::from_secs(1);

    /// Tracker anchored at a known boottime and a real (but fixed)
    /// monotonic instant, so each test advances both clocks by hand.
    fn anchored_tracker() -> (SleepTracker, Duration, Instant) {
        let boot = Duration::from_secs(1_000);
        let mono = Instant::now();
        (SleepTracker::anchored(boot, mono), boot, mono)
    }

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

    /// Both clocks advancing in lockstep is ordinary awake time, no
    /// matter how much of it passes. This also encodes the NTP
    /// property: a stepped wall clock is invisible here because the
    /// detector never reads the wall clock at all.
    #[test]
    fn equal_boot_and_mono_advance_is_not_suspend() {
        let (t, boot, mono) = anchored_tracker();
        assert_eq!(t.observe(boot + 3_600 * SEC, mono + 3_600 * SEC), 0);
        assert_eq!(t.accumulated_suspended_seconds(), 0);
    }

    /// Boottime running ahead of monotonic is the suspend signature:
    /// 10s of runtime plus a 90s suspend shows up as a 100s boottime
    /// advance against a 10s monotonic advance.
    #[test]
    fn boottime_ahead_of_monotonic_records_suspend() {
        let (t, boot, mono) = anchored_tracker();
        assert_eq!(t.observe(boot + 100 * SEC, mono + 10 * SEC), 90);
        assert_eq!(t.accumulated_suspended_seconds(), 90);
    }

    #[test]
    fn sub_threshold_drift_is_ignored() {
        let (t, boot, mono) = anchored_tracker();
        assert_eq!(t.observe(boot + 14 * SEC, mono + 10 * SEC), 0);
        assert_eq!(t.accumulated_suspended_seconds(), 0);
    }

    #[test]
    fn suspends_accumulate_across_observations() {
        let (t, boot, mono) = anchored_tracker();
        assert_eq!(t.observe(boot + 70 * SEC, mono + 10 * SEC), 60);
        // A normal awake stretch in between must not disturb the total.
        assert_eq!(t.observe(boot + 100 * SEC, mono + 40 * SEC), 0);
        assert_eq!(t.observe(boot + 400 * SEC, mono + 100 * SEC), 240);
        assert_eq!(t.accumulated_suspended_seconds(), 300);
    }

    /// Monotonic can't outrun boottime on real hardware; if read
    /// skew ever makes it look that way, clamp to zero rather than
    /// crediting negative suspend.
    #[test]
    fn monotonic_ahead_of_boottime_clamps_to_zero() {
        let (t, boot, mono) = anchored_tracker();
        assert_eq!(t.observe(boot + 5 * SEC, mono + 10 * SEC), 0);
        assert_eq!(t.accumulated_suspended_seconds(), 0);
    }
}
