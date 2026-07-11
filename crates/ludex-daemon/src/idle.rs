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

    /// Snapshot for a session opening now: `(closed interval count,
    /// seconds already elapsed on any currently-open idle interval)`.
    ///
    /// Both are read under a single lock so they stay mutually
    /// consistent. If the open interval sealed between two separate
    /// reads, the count would advance while the open-elapsed dropped to
    /// zero, and the just-sealed interval's pre-session idle would leak
    /// into the session. Feed the pair straight into
    /// [`Self::billable_idle_seconds_since`].
    #[must_use]
    pub fn session_start_baseline(&self) -> (usize, i64) {
        let s = self.state.lock().expect("idle tracker mutex poisoned");
        let count = s.closed_intervals_seconds.len();
        let open_elapsed = s.since.map_or(0, seconds_since);
        (count, open_elapsed)
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
    /// When a session starts while the user is already idle, only the
    /// portion of that open interval that elapses *during* the session
    /// counts against it. `baseline_open_seconds` is how much of the
    /// open interval had already elapsed at session start (from
    /// [`Self::session_start_baseline`]). For the first post-baseline
    /// interval — the one that was open at session start, whether it is
    /// still open or has since sealed into
    /// `closed_intervals_seconds[baseline_count]` — the session bills
    /// `max(0, duration − max(baseline_open, grace))`: the pre-session
    /// elapsed is never charged, and the cutscene grace is forgiven once
    /// for the whole natural interval rather than once per session that
    /// observes it. Without this, idle from before the session (and,
    /// when an interval spans two adjacent sessions, the same wall-clock
    /// idle) would be billed to the session. It is not the rare case it
    /// looks like: a gamepad / Big Picture launch doesn't reset the
    /// compositor idle timer, so a session can routinely open
    /// mid-idle-interval.
    #[must_use]
    pub fn billable_idle_seconds_since(
        &self,
        baseline_count: usize,
        baseline_open_seconds: i64,
        grace_seconds: i64,
    ) -> i64 {
        let s = self.state.lock().expect("idle tracker mutex poisoned");
        let mut total: i64 = 0;
        // The first interval after the baseline is exactly the one that
        // was open when the session started (a new interval cannot begin
        // until the open one seals), whether it is still open or now
        // closed. Forgive `grace` once for that whole natural interval,
        // not once per session that observes it: subtract
        // `max(baseline_open, grace)`. When the pre-session elapsed
        // already exceeds the grace the cutscene window was spent before
        // the session, so the in-session tail bills in full; otherwise
        // only the unused remainder of the grace is forgiven in-session.
        let mut first = true;
        for dur in s.closed_intervals_seconds.iter().skip(baseline_count) {
            let forgiven = if first { baseline_open_seconds.max(grace_seconds) } else { grace_seconds };
            total = total.saturating_add((*dur - forgiven).max(0));
            first = false;
        }
        if let Some(since) = s.since {
            let dur = seconds_since(since);
            let forgiven = if first { baseline_open_seconds.max(grace_seconds) } else { grace_seconds };
            total = total.saturating_add((dur - forgiven).max(0));
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

/// Proxy for a login session's `IdleHint` property.
///
/// No `default_path` is set on purpose: the object path must be the
/// *canonical* `/org/freedesktop/login1/session/_NN` for this session,
/// resolved at runtime (see [`resolve_session_path`]). The obvious
/// `/session/auto` alias resolves fine for method calls but never
/// emits `PropertiesChanged`, so a proxy built on it silently never
/// sees an `IdleHint` transition.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
trait LogindSession {
    #[zbus(property, name = "IdleHint")]
    fn idle_hint(&self) -> zbus::Result<bool>;
}

/// Proxy for the logind manager, used to resolve the canonical session
/// object path.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait LogindManager {
    fn get_session(&self, session_id: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
    fn get_user(&self, uid: u32) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

/// Proxy for a logind user object, used to read its primary graphical
/// (`Display`) session.
#[zbus::proxy(
    interface = "org.freedesktop.login1.User",
    default_service = "org.freedesktop.login1"
)]
trait LogindUser {
    /// `(session_id, object_path)` of the user's primary graphical
    /// session, or `("", "/")` when the user has none.
    #[zbus(property)]
    fn display(&self) -> zbus::Result<(String, zbus::zvariant::OwnedObjectPath)>;
}

/// Resolve the canonical logind session object path to watch for idle
/// transitions.
///
/// Two strategies, in order:
///
/// 1. `GetSession($XDG_SESSION_ID)` — the session the daemon was
///    launched into. This is the normal path: `systemd --user` exports
///    `XDG_SESSION_ID` into the user manager's environment on both KDE
///    and GNOME, so the daemon inherits it.
/// 2. `GetUser(getuid())` → the user object's `Display` property — the
///    user's primary graphical session. This is the fallback for when
///    `XDG_SESSION_ID` is absent. It is deliberately *not*
///    `GetSessionByPID(getpid())`: a `systemd --user` daemon runs under
///    `user@UID.service`, not a `session-NN.scope`, so logind cannot map
///    its PID to a session at all — the user's `Display` session is the
///    only thing that resolves for a user-service process.
///
/// Both return the concrete `/session/_NN` path that actually emits
/// `PropertiesChanged`, never the `/session/auto` alias (which resolves
/// for method calls but is silent for signals). Returns an error when
/// neither strategy finds a session; the caller degrades to "idle
/// tracking disabled" with a warning.
async fn resolve_session_path(conn: &zbus::Connection) -> Result<zbus::zvariant::OwnedObjectPath> {
    let manager = LogindManagerProxy::new(conn)
        .await
        .context("construct logind manager proxy")?;

    if let Ok(id) = std::env::var("XDG_SESSION_ID") {
        if !id.is_empty() {
            if let Ok(path) = manager.get_session(&id).await {
                return Ok(path);
            }
        }
    }

    let uid = rustix::process::getuid().as_raw();
    let user_path = manager
        .get_user(uid)
        .await
        .context("resolve logind user object")?;
    let user = LogindUserProxy::builder(conn)
        .path(user_path)
        .context("set logind user path")?
        .build()
        .await
        .context("construct logind user proxy")?;
    let (session_id, path) = user
        .display()
        .await
        .context("read logind user Display session")?;
    if session_id.is_empty() {
        anyhow::bail!("logind user has no graphical Display session");
    }
    Ok(path)
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
    let session_path = match resolve_session_path(&conn).await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "could not resolve logind session; idle tracking disabled");
            return Ok(());
        }
    };
    debug!(path = %session_path.as_str(), "resolved logind session path");
    let proxy = LogindSessionProxy::builder(&conn)
        .path(session_path)
        .context("set logind session path")?
        .build()
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
        assert_eq!(t.billable_idle_seconds_since(0, 0, 300), 0);
    }

    #[test]
    fn billable_long_interval_is_billed_minus_grace() {
        let t = IdleTracker::new();
        t.record_idle_interval(30 * 60); // 30 min AFK
                                         // Grace 5 min: billable = 25 min.
        assert_eq!(t.billable_idle_seconds_since(0, 0, 5 * 60), 25 * 60);
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
        assert_eq!(t.billable_idle_seconds_since(baseline, 0, 5 * 60), 5 * 60);
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
        assert_eq!(t.billable_idle_seconds_since(0, 0, 5 * 60), 0);
    }

    /// Regression guard for IDLE-1: the resolved logind session path
    /// must be the canonical `/session/_NN`, never the `/session/auto`
    /// alias, because only the canonical path emits
    /// `PropertiesChanged` for `IdleHint`. Subscribing on the alias
    /// left idle tracking silently dead for every session.
    ///
    /// Requires a live system bus with a logind session (present on a
    /// desktop and in CI-with-logind). Skips cleanly otherwise so the
    /// suite still passes in a bare container.
    #[tokio::test]
    async fn resolved_session_path_is_canonical_not_auto() {
        let Ok(conn) = zbus::Connection::system().await else {
            eprintln!("no system bus; skipping logind resolution test");
            return;
        };
        let path = match resolve_session_path(&conn).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("no logind session ({e}); skipping resolution test");
                return;
            }
        };
        let path = path.as_str();
        let suffix = path
            .strip_prefix("/org/freedesktop/login1/session/")
            .unwrap_or_else(|| panic!("unexpected session path shape: {path}"));
        // The real regression is subscribing on the `auto`/`self`
        // aliases, which resolve for method calls but never emit
        // `PropertiesChanged`. Any concrete session id is fine — logind
        // escapes leading digits (`2` → `_32`) but a named session can
        // be alphanumeric, so don't over-constrain the shape.
        assert!(
            !suffix.is_empty() && suffix != "auto" && suffix != "self",
            "resolver returned a non-emitting alias instead of a concrete session: {path}"
        );
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
        let billable = t.billable_idle_seconds_since(0, 0, 5 * 60);
        assert_eq!(billable, 5 * 60);
    }

    /// IDLE-2: a session that opens while the user is already idle must
    /// not be billed for the pre-session portion of that open interval,
    /// and the cutscene grace is forgiven once for the whole natural
    /// interval. The interval seals at 300s total; 100s elapsed before
    /// the session, and that 100s already exceeds the 30s grace, so the
    /// 200s in-session tail bills in full: 300 − max(100, 30) = 200.
    /// Before the fix this billed (300 − 30) = 270, charging the 100s of
    /// pre-session idle to the session.
    #[test]
    fn billable_subtracts_pre_session_open_interval() {
        let t = IdleTracker::new();
        // Session opened with an idle interval already 100s in; that
        // same interval later seals at 300s total.
        t.record_idle_interval(300);
        assert_eq!(t.billable_idle_seconds_since(0, 100, 30), 200);
    }

    /// When the pre-session elapsed is *under* the grace, only the
    /// unused remainder of the grace is forgiven in-session — the grace
    /// still applies exactly once to the natural interval. Interval
    /// seals at 300s, 10s pre-session, grace 30: 300 − max(10, 30) = 270.
    #[test]
    fn billable_open_baseline_below_grace_forgives_grace_once() {
        let t = IdleTracker::new();
        t.record_idle_interval(300);
        assert_eq!(t.billable_idle_seconds_since(0, 10, 30), 270);
    }

    /// The pre-session subtraction applies only to the *first*
    /// post-baseline interval (the one open at session start); later
    /// intervals are wholly in-session. Grace 0 isolates the
    /// subtraction: (300 − 100) + 50 = 250.
    #[test]
    fn billable_pre_session_open_affects_only_first_interval() {
        let t = IdleTracker::new();
        t.record_idle_interval(300); // was open at start; sealed
        t.record_idle_interval(50); // wholly in-session
        assert_eq!(t.billable_idle_seconds_since(0, 100, 0), 250);
    }

    /// A zero open-baseline (the common case: not idle at session start)
    /// leaves billing exactly as before — regression guard.
    #[test]
    fn billable_zero_open_baseline_is_unchanged() {
        let t = IdleTracker::new();
        t.record_idle_interval(30 * 60);
        assert_eq!(
            t.billable_idle_seconds_since(0, 0, 5 * 60),
            25 * 60,
            "with nothing open at start, billing is plain interval-minus-grace",
        );
    }
}
