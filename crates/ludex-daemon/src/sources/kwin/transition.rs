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

use crate::gate::{AcceptedProcess, GateDecision};

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
pub(super) fn next_action(
    state: FgState,
    meta: &ForegroundMeta,
    decision: GateDecision,
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
            // and session continues uninterrupted.
            FgState::Tracked(af) => Outcome {
                events: Vec::new(),
                state: FgState::TrackedBackgrounded(af),
                timer: TimerOp::Start,
            },
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
pub(super) fn on_grace_timeout(state: FgState) -> Outcome {
    match state {
        FgState::TrackedBackgrounded(af) => {
            let key = af.key.clone();
            Outcome {
                events: vec![TransitionEvent::Stop { key }],
                state: FgState::NotTracked,
                timer: TimerOp::NoChange,
            }
        }
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

fn native_key(accepted: &AcceptedProcess) -> GameKey {
    GameKey::native(accepted.executable_path.to_string_lossy().into_owned())
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
        GateDecision::Accept(AcceptedProcess {
            executable_path: PathBuf::from(path),
            graphics_libraries: GraphicsLibraries {
                vulkan: true,
                ..Default::default()
            },
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

    #[test]
    fn not_tracked_plus_reject_stays_not_tracked() {
        let o = next_action(FgState::NotTracked, &meta(100, "Firefox", ""), rejected());
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
    fn tracked_same_pid_is_noop() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::Tracked(prev.clone()),
            &meta(100, "Celeste", "Chapter 1"),
            accepted("/opt/celeste/Celeste"),
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
        );
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::TrackedBackgrounded(prev));
        assert_eq!(o.timer, TimerOp::Start);
    }

    #[test]
    fn tracked_plus_accept_different_game_switches_without_grace() {
        let prev = af(100, "/opt/celeste/Celeste");
        let o = next_action(
            FgState::Tracked(prev.clone()),
            &meta(200, "Factorio", "Factorio"),
            accepted("/opt/factorio/factorio"),
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
        let o = on_grace_timeout(FgState::TrackedBackgrounded(prev.clone()));
        assert_eq!(
            o.events,
            vec![TransitionEvent::Stop {
                key: prev.key.clone()
            }]
        );
        assert_eq!(o.state, FgState::NotTracked);
    }

    #[test]
    fn grace_timeout_from_other_states_is_noop() {
        let o = on_grace_timeout(FgState::NotTracked);
        assert_eq!(o.events, vec![]);
        assert_eq!(o.state, FgState::NotTracked);

        let af = af(100, "/opt/celeste/Celeste");
        let o = on_grace_timeout(FgState::Tracked(af.clone()));
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
        );
        assert!(matches!(
            &o.events[0],
            TransitionEvent::Start { display_name, .. } if display_name == "Window Caption"
        ));

        let o2 = next_action(
            FgState::NotTracked,
            &meta(100, "", ""),
            accepted("/opt/foo/foo_bin"),
        );
        assert!(matches!(
            &o2.events[0],
            TransitionEvent::Start { display_name, .. } if display_name == "/opt/foo/foo_bin"
        ));
    }
}
