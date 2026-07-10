//! Pure state-transition logic for the foreground source.
//!
//! The gate answers "is this PID a game?" for a single window;
//! this module answers "given what we were tracking and what is
//! foreground now, what session-level events should fire?"
//!
//! A grace window sits between "tracked game lost foreground" and
//! "emit Stop": if the tracked window returns within the grace
//! period, no Stop is emitted and the session continues. Without
//! that window every alt-tab to the browser and back would split
//! the session in two and inflate the application's run count —
//! the same pattern that left legacy trackers reporting thousands
//! of "runs" on games the user genuinely played a few hundred
//! times. Switching to a *different* game bypasses the grace
//! window (clear user intent).
//!
//! This module is `async`-free and I/O-free so every state
//! transition is unit-testable without a compositor, a clock, or a
//! tokio runtime. The runner (`source.rs`) holds the timer.

use std::path::PathBuf;

use ludex_core::GameKey;

use crate::gate::{AcceptedProcess, GateDecision, LauncherAttribution};

/// In-memory record of the foreground window the source has
/// decided to track — tracked both while it's foreground and while
/// it's temporarily backgrounded inside the grace window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcceptedForeground {
    /// PID of the accepted window's process.
    pub pid: u32,
    /// Key the `Started` was emitted under; stored so `Stopped`
    /// uses the same identity even if we lose the exe path later.
    pub key: GameKey,
    /// Canonical executable path (for logging, enrichment
    /// bootstrapping).
    pub executable_path: PathBuf,
}

/// Metadata the foreground source collected alongside the gate
/// decision.
#[derive(Debug, Clone)]
pub(super) struct ForegroundMeta {
    /// PID of the window's process.
    pub pid: u32,
    /// `Window.resourceClass` from KWin — often a game's short
    /// name.
    pub resource_class: String,
    /// `Window.caption` from KWin — window title. Fallback
    /// display name.
    pub caption: String,
}

/// What the source is currently tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FgState {
    /// No game is currently tracked. Activations for non-game
    /// windows stay here; for accepted windows move to Tracked.
    NotTracked,
    /// A game is active and foreground.
    Tracked(AcceptedForeground),
    /// A game was foreground and is now backgrounded; a grace
    /// timer is running. If the tracked PID returns to the
    /// foreground before the timer fires, the session resumes
    /// without a Stop ever being emitted.
    TrackedBackgrounded(AcceptedForeground),
}

impl FgState {
    /// Borrow the currently-tracked foreground, if any. Shared
    /// between Tracked and TrackedBackgrounded — the distinction
    /// only matters for event emission and timer lifecycle, not
    /// for "what game are we tracking right now".
    pub(super) fn current(&self) -> Option<&AcceptedForeground> {
        match self {
            Self::NotTracked => None,
            Self::Tracked(af) | Self::TrackedBackgrounded(af) => Some(af),
        }
    }
}

/// What the runner should do with its grace timer after applying
/// an [`Outcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimerOp {
    /// Keep the timer as-is (running or idle, whichever it was).
    /// Used when the state change is unrelated to the grace
    /// window, or when a TrackedBackgrounded state stays
    /// TrackedBackgrounded (we don't reset the timer on repeated
    /// rejections — the grace period is about "how long until the
    /// tracked window returns", not "how long since the last
    /// activation").
    NoChange,
    /// Start a fresh timer counting down the grace period. Emitted
    /// when transitioning Tracked → TrackedBackgrounded.
    Start,
    /// Cancel any running timer. Emitted when TrackedBackgrounded
    /// resolves — either by the tracked window coming back, the
    /// user switching to another game, or the tracked process
    /// exiting.
    Cancel,
}

/// A single event for the source to emit on its shared channel.
/// Split out from `ludex_daemon::event::GameEvent` so the pure
/// logic doesn't need to synthesise timestamps; the runner stamps
/// `OffsetDateTime::now_utc()` when it relays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TransitionEvent {
    Start {
        key: GameKey,
        executable_path: PathBuf,
        display_name: String,
    },
    Stop {
        key: GameKey,
    },
}

/// The computed result of a state transition: events to emit, the
/// new state, and what to do with the grace timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Outcome {
    pub events: Vec<TransitionEvent>,
    pub state: FgState,
    pub timer: TimerOp,
}

impl Outcome {
    fn noop(state: FgState) -> Self {
        Self {
            events: Vec::new(),
            state,
            timer: TimerOp::NoChange,
        }
    }
}

/// Handle a KWin foreground-window activation. Returns the
/// events to emit, the new state, and the timer action.
///
/// `pause_when_backgrounded` is read per-call so live-reload works:
/// when `false`, a reject decision doesn't transition a tracked
/// game into the backgrounded state at all, so the grace timer
/// never arms and sessions only end on process exit. When `true`
/// (default) the grace window described at the top of the module
/// applies.
pub(super) fn next_action(
    state: FgState,
    meta: &ForegroundMeta,
    decision: GateDecision,
    pause_when_backgrounded: bool,
) -> Outcome {
    // Activation for the same pid we're already tracking is a no-
    // op while foreground, and is "return to foreground" while
    // backgrounded — the latter cancels the grace timer.
    if state.current().is_some_and(|af| af.pid == meta.pid) {
        if let FgState::TrackedBackgrounded(af) = state {
            return Outcome {
                events: Vec::new(),
                state: FgState::Tracked(af),
                timer: TimerOp::Cancel,
            };
        }
        // Already tracked + foreground, nothing to do. (NotTracked
        // is unreachable here because `current()` returned `Some`.)
        return Outcome::noop(state);
    }

    match decision {
        GateDecision::Accept(accepted) => {
            let new_af = AcceptedForeground {
                pid: meta.pid,
                key: native_key(&accepted),
                executable_path: accepted.executable_path.clone(),
            };
            let start_event = TransitionEvent::Start {
                key: new_af.key.clone(),
                executable_path: accepted.executable_path,
                display_name: display_name_from(meta, &new_af.key),
            };
            match state {
                FgState::NotTracked => Outcome {
                    events: vec![start_event],
                    state: FgState::Tracked(new_af),
                    timer: TimerOp::NoChange,
                },
                // Switching to a different game: emit Stop for the
                // previous tracked, Start for the new one. No
                // grace — the user is clearly engaging a different
                // game, not briefly checking a browser tab.
                FgState::Tracked(prev) => {
                    let events = vec![
                        TransitionEvent::Stop {
                            key: prev.key.clone(),
                        },
                        start_event,
                    ];
                    Outcome {
                        events,
                        state: FgState::Tracked(new_af),
                        timer: TimerOp::NoChange,
                    }
                }
                // Same but from the backgrounded state: cancel the
                // pending grace timer so we don't fire Stop twice.
                FgState::TrackedBackgrounded(prev) => {
                    let events = vec![
                        TransitionEvent::Stop {
                            key: prev.key.clone(),
                        },
                        start_event,
                    ];
                    Outcome {
                        events,
                        state: FgState::Tracked(new_af),
                        timer: TimerOp::Cancel,
                    }
                }
            }
        }
        GateDecision::Reject(_) => match state {
            FgState::NotTracked => Outcome::noop(FgState::NotTracked),
            // Tracked → TrackedBackgrounded: start the grace timer.
            // If the tracked window returns within the grace
            // period, the same-pid path above cancels the timer
            // and session continues uninterrupted. When pause-on-
            // focus-loss is disabled, the tracked window losing
            // focus is not a session-end signal at all — stay in
            // Tracked and let the session run until process exit.
            FgState::Tracked(af) => {
                if pause_when_backgrounded {
                    Outcome {
                        events: Vec::new(),
                        state: FgState::TrackedBackgrounded(af),
                        timer: TimerOp::Start,
                    }
                } else {
                    Outcome::noop(FgState::Tracked(af))
                }
            }
            // Already backgrounded + another non-game activation:
            // don't reset the timer. The grace period is about how
            // long the tracked window has been absent, not how
            // long since the last unrelated activation.
            FgState::TrackedBackgrounded(af) => Outcome::noop(FgState::TrackedBackgrounded(af)),
        },
    }
}

/// Called by the runner when the grace timer fires. Only
/// meaningful when the state is TrackedBackgrounded; other states
/// no-op (the runner shouldn't be polling the timer in those
/// states, but defensive behaviour is free).
///
/// `pause_when_backgrounded` is consulted here too: if the user
/// toggled the setting off while a timer was already armed (from
/// a previous Tracked → TrackedBackgrounded transition), honour
/// the new intent and resume Tracked rather than closing the
/// session when the leftover timer fires.
pub(super) fn on_grace_timeout(state: FgState, pause_when_backgrounded: bool) -> Outcome {
    match state {
        FgState::TrackedBackgrounded(af) if pause_when_backgrounded => {
            let key = af.key.clone();
            Outcome {
                events: vec![TransitionEvent::Stop { key }],
                state: FgState::NotTracked,
                timer: TimerOp::NoChange,
            }
        }
        // Pause setting turned off while a leftover timer was
        // still armed from a previous Tracked → Backgrounded
        // transition: collapse back to Tracked silently so the
        // session keeps running.
        FgState::TrackedBackgrounded(af) => Outcome {
            events: Vec::new(),
            state: FgState::Tracked(af),
            timer: TimerOp::NoChange,
        },
        _ => Outcome::noop(state),
    }
}

/// Called by the runner when a `pidfd`-watched process exits. If
/// the exiting pid is the one we're tracking (in either state),
/// close the session immediately — don't wait for the grace
/// window, the process is gone.
pub(super) fn on_tracked_exit(state: FgState, exited_pid: u32) -> Outcome {
    match state.current() {
        Some(af) if af.pid == exited_pid => Outcome {
            events: vec![TransitionEvent::Stop {
                key: af.key.clone(),
            }],
            state: FgState::NotTracked,
            timer: TimerOp::Cancel,
        },
        _ => Outcome::noop(state),
    }
}

/// Build a [`GameKey`] for an accepted process. Foreground-source
/// launcher attribution wins over the executable path: the wine /
/// Proton preloader path is shared across games (every Lutris game on
/// a runner, or every Heroic game on a given wine variant, resolves to
/// the same preloader), while a launcher's own canonical id — Heroic's
/// `HEROIC_APP_NAME` or Lutris's `LUTRIS_GAME_UUID` — is invariant and
/// unique per game, which is what we want to key sessions against.
/// Falls back to a `Native` key from the executable path when no
/// attribution is available.
fn native_key(accepted: &AcceptedProcess) -> GameKey {
    match &accepted.attribution {
        Some(LauncherAttribution::Heroic { app_name }) => GameKey::heroic(app_name.clone()),
        Some(LauncherAttribution::Lutris { uuid }) => GameKey::lutris(uuid.clone()),
        None => GameKey::native(accepted.executable_path.to_string_lossy().into_owned()),
    }
}

fn display_name_from(meta: &ForegroundMeta, fallback_key: &GameKey) -> String {
    // Preference order: window resource class → window caption →
    // nothing better than the path we already have. The enrich
    // cascade refines this on a background task.
    if !meta.resource_class.trim().is_empty() {
        return meta.resource_class.clone();
    }
    if !meta.caption.trim().is_empty() {
        return meta.caption.clone();
    }
    // Without resource_class or caption, fall back to the key's
    // launcher_id — for Native keys that's the exe path, which is
    // at least unambiguous.
    fallback_key.launcher_id.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RejectionReason;
    use crate::proc::maps::GraphicsLibraries;

    fn accepted(path: &str) -> GateDecision {
        accepted_with(path, None)
    }

    fn accepted_with(path: &str, attribution: Option<LauncherAttribution>) -> GateDecision {
        GateDecision::Accept(AcceptedProcess {
            executable_path: PathBuf::from(path),
            graphics_libraries: GraphicsLibraries {
                vulkan: true,
                ..Default::default()
            },
            attribution,
        })
    }

    fn rejected() -> GateDecision {
        GateDecision::Reject(RejectionReason::NoGraphicsLibrary)
    }

    fn meta(pid: u32, class: &str, caption: &str) -> ForegroundMeta {
        ForegroundMeta {
            pid,
            resource_class: class.into(),
            caption: caption.into(),
        }
    }

    fn af(pid: u32, path: &str) -> AcceptedForeground {
        AcceptedForeground {
            pid,
            key: GameKey::native(path),
            executable_path: PathBuf::from(path),
        }
    }

    /// Default: the historical alt-tab pause behaviour is on.
    const PAUSE: bool = true;

    #[test]
    fn not_tracked_plus_reject_stays_not_tracked() {
        let o = next_action(
            FgState::NotTracked,
            &meta(100, "Firefox", ""),
            rejected(),
            PAUSE,
        );
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::NotTracked);
        assert_eq!(o.timer, TimerOp::NoChange);
    }

    #[test]
    fn not_tracked_plus_accept_emits_start() {
        let o = next_action(
            FgState::NotTracked,
            &meta(100, "Celeste", "Celeste"),
            accepted("/opt/celeste/Celeste"),
            PAUSE,
        );
        assert_eq!(o.events.len(), 1);
        assert!(matches!(
            &o.events[0],
            TransitionEvent::Start { display_name, .. } if display_name == "Celeste"
        ));
        assert_eq!(o.timer, TimerOp::NoChange);
        assert!(matches!(o.state, FgState::Tracked(ref a) if a.pid == 100));
    }

    #[test]
    fn heroic_attribution_keys_session_by_app_name() {
        // Heroic-launched games inherit a wine/Proton preloader path
        // as `executable_path` — varies per wine variant the user
        // picked in Heroic, so it can't be a stable session key.
        // When the gate surfaces a `Heroic` attribution, the key
        // must come from `HEROIC_APP_NAME` instead, so re-launches
        // (and wine-version switches) collapse onto the same row.
        let o = next_action(
            FgState::NotTracked,
            &meta(200, "steam_app_0", "Builder's Journey"),
            accepted_with(
                "/home/u/.config/heroic/tools/proton/Proton-GE/files/bin/wine64-preloader",
                Some(LauncherAttribution::Heroic {
                    app_name: "deadbeef-epic-guid".to_owned(),
                }),
            ),
            PAUSE,
        );
        assert_eq!(o.events.len(), 1);
        let TransitionEvent::Start { key, .. } = &o.events[0] else {
            panic!("expected Start event, got {:?}", o.events[0]);
        };
        assert_eq!(*key, GameKey::heroic("deadbeef-epic-guid"));
        assert!(
            matches!(o.state, FgState::Tracked(ref a) if a.key == GameKey::heroic("deadbeef-epic-guid"))
        );
    }

    #[test]
    fn lutris_attribution_keys_session_by_uuid() {
        // Every Lutris/bare-Wine game on a runner shares the same
        // wine64-preloader `executable_path` — keying by it would
        // collapse every Lutris game onto one application row
        // (GATE-2). When the gate surfaces a `Lutris` attribution,
        // the key must come from `LUTRIS_GAME_UUID` instead.
        let o = next_action(
            FgState::NotTracked,
            &meta(200, "wine64-preloader", "Some Game"),
            accepted_with(
                "/home/u/.local/share/lutris/runners/wine/lutris-fshack/bin/wine64-preloader",
                Some(LauncherAttribution::Lutris {
                    uuid: "abc-123".to_owned(),
                }),
            ),
            PAUSE,
        );
        assert_eq!(o.events.len(), 1);
        let TransitionEvent::Start { key, .. } = &o.events[0] else {
            panic!("expected Start event, got {:?}", o.events[0]);
        };
        assert_eq!(*key, GameKey::lutris("abc-123"));
        assert!(matches!(o.state, FgState::Tracked(ref a) if a.key == GameKey::lutris("abc-123")));
    }

    #[test]
    fn tracked_same_pid_is_noop() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::Tracked(prev.clone()),
            &meta(100, "Celeste", "Chapter 1"),
            accepted("/opt/celeste/Celeste"),
            PAUSE,
        );
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::Tracked(prev));
        assert_eq!(o.timer, TimerOp::NoChange);
    }

    #[test]
    fn tracked_plus_reject_starts_grace_timer() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::Tracked(prev.clone()),
            &meta(200, "Firefox", ""),
            rejected(),
            PAUSE,
        );
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::TrackedBackgrounded(prev));
        assert_eq!(o.timer, TimerOp::Start);
    }

    #[test]
    fn tracked_plus_reject_stays_tracked_when_pause_disabled() {
        // Pause-on-focus-loss disabled: the tracked window going
        // out of focus does not transition to Backgrounded and
        // does not start a grace timer. Session persists until
        // the process exits.
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::Tracked(prev.clone()),
            &meta(200, "Firefox", ""),
            rejected(),
            false,
        );
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::Tracked(prev));
        assert_eq!(o.timer, TimerOp::NoChange);
    }

    #[test]
    fn tracked_plus_accept_different_game_switches_without_grace() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::Tracked(prev.clone()),
            &meta(200, "Factorio", "Factorio"),
            accepted("/opt/factorio/factorio"),
            PAUSE,
        );
        assert_eq!(o.events.len(), 2);
        assert!(matches!(
            &o.events[0],
            TransitionEvent::Stop { key } if key == &prev.key
        ));
        assert!(matches!(&o.events[1], TransitionEvent::Start { .. }));
        assert!(matches!(o.state, FgState::Tracked(ref a) if a.pid == 200));
        assert_eq!(o.timer, TimerOp::NoChange);
    }

    #[test]
    fn backgrounded_plus_same_pid_cancels_grace_and_resumes() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::TrackedBackgrounded(prev.clone()),
            &meta(100, "Celeste", "Chapter 1"),
            accepted("/opt/celeste/Celeste"),
            PAUSE,
        );
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::Tracked(prev));
        assert_eq!(o.timer, TimerOp::Cancel);
    }

    #[test]
    fn backgrounded_plus_another_non_game_does_not_reset_timer() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::TrackedBackgrounded(prev.clone()),
            &meta(300, "Telegram", ""),
            rejected(),
            PAUSE,
        );
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::TrackedBackgrounded(prev));
        assert_eq!(o.timer, TimerOp::NoChange);
    }

    #[test]
    fn backgrounded_plus_different_game_switches_and_cancels_grace() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::TrackedBackgrounded(prev.clone()),
            &meta(200, "Factorio", ""),
            accepted("/opt/factorio/factorio"),
            PAUSE,
        );
        assert_eq!(o.events.len(), 2);
        assert!(matches!(
            &o.events[0],
            TransitionEvent::Stop { key } if key == &prev.key
        ));
        assert!(matches!(&o.events[1], TransitionEvent::Start { .. }));
        assert_eq!(o.timer, TimerOp::Cancel);
    }

    #[test]
    fn grace_timeout_from_backgrounded_emits_stop() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = on_grace_timeout(FgState::TrackedBackgrounded(prev.clone()), PAUSE);
        assert_eq!(
            o.events,
            vec![TransitionEvent::Stop {
                key: prev.key.clone()
            }]
        );
        assert_eq!(o.state, FgState::NotTracked);
    }

    #[test]
    fn grace_timeout_with_pause_disabled_resumes_tracked() {
        // User toggled pause off after a Tracked → Backgrounded
        // transition already armed the timer. When it fires we
        // should honour the new intent and resume, not close.
        let prev = af(100, "/opt/celeste/Celeste");
        let o = on_grace_timeout(FgState::TrackedBackgrounded(prev.clone()), false);
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::Tracked(prev));
        assert_eq!(o.timer, TimerOp::NoChange);
    }

    #[test]
    fn grace_timeout_from_other_states_is_noop() {
        let o = on_grace_timeout(FgState::NotTracked, PAUSE);
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::NotTracked);

        let af = af(100, "/opt/celeste/Celeste");
        let o = on_grace_timeout(FgState::Tracked(af.clone()), PAUSE);
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::Tracked(af));
    }

    #[test]
    fn tracked_exit_emits_stop_and_cancels_timer() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = on_tracked_exit(FgState::Tracked(prev.clone()), 100);
        assert_eq!(
            o.events,
            vec![TransitionEvent::Stop {
                key: prev.key.clone()
            }]
        );
        assert_eq!(o.state, FgState::NotTracked);
        assert_eq!(o.timer, TimerOp::Cancel);
    }

    #[test]
    fn backgrounded_exit_emits_stop_immediately_no_grace_wait() {
        // Even in TrackedBackgrounded with a pending grace timer,
        // process exit closes the session now — grace is about
        // "might come back", not about a dead process.
        let prev = af(100, "/opt/celeste/Celeste");
        let o = on_tracked_exit(FgState::TrackedBackgrounded(prev.clone()), 100);
        assert_eq!(
            o.events,
            vec![TransitionEvent::Stop {
                key: prev.key.clone()
            }]
        );
        assert_eq!(o.state, FgState::NotTracked);
        assert_eq!(o.timer, TimerOp::Cancel);
    }

    #[test]
    fn exit_for_unrelated_pid_is_noop() {
        let prev = af(100, "/opt/celeste/Celeste");
        let state = FgState::Tracked(prev.clone());
        let o = on_tracked_exit(state.clone(), 999);
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, state);
    }

    #[test]
    fn display_name_falls_back_to_caption_then_launcher_id() {
        let o = next_action(
            FgState::NotTracked,
            &meta(100, "", "Window Caption"),
            accepted("/opt/foo/foo"),
            PAUSE,
        );
        assert!(matches!(
            &o.events[0],
            TransitionEvent::Start { display_name, .. } if display_name == "Window Caption"
        ));

        let o2 = next_action(
            FgState::NotTracked,
            &meta(100, "", ""),
            accepted("/opt/foo/foo_bin"),
            PAUSE,
        );
        assert!(matches!(
            &o2.events[0],
            TransitionEvent::Start { display_name, .. } if display_name == "/opt/foo/foo_bin"
        ));
    }
}
