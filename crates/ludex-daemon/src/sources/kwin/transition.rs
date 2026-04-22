//! Pure state-transition logic for the foreground source.
//!
//! The gate decides whether a given PID is a game; this module decides
//! what session-level events to emit given the *transition* from the
//! previously-accepted foreground (if any) to the current one.
//!
//! Split out so the decision is unit-testable without `/proc`, without
//! zbus, and without a live KWin.

use std::path::PathBuf;

use ludex_core::GameKey;

use crate::gate::{AcceptedProcess, GateDecision};

/// In-memory record of the foreground window the source is currently
/// tracking (i.e. has emitted `Started` for and not yet `Stopped`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcceptedForeground {
    /// PID of the accepted window's process.
    pub pid: u32,
    /// Key the `Started` was emitted under; stored so `Stopped` uses
    /// the same identity even if we lose the exe path later.
    pub key: GameKey,
    /// Canonical executable path (for logging, enrichment bootstrapping).
    pub executable_path: PathBuf,
}

/// Metadata the foreground source collected alongside the gate decision.
#[derive(Debug, Clone)]
pub(super) struct ForegroundMeta {
    /// PID of the window's process.
    pub pid: u32,
    /// `Window.resourceClass` from KWin — often a game's short name.
    pub resource_class: String,
    /// `Window.caption` from KWin — window title. Fallback display name.
    pub caption: String,
}

/// What the source should do as a result of a foreground change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Transition {
    /// No session-level change required (new foreground is the same
    /// PID as the current tracked one, or a rejected non-game window
    /// with nothing currently tracked).
    None,
    /// Emit `Stopped` for the previously-tracked foreground.
    Stop {
        /// Key of the session to stop.
        key: GameKey,
    },
    /// Emit `Started` for a newly-accepted foreground. No stop
    /// required (nothing was tracked).
    Start {
        /// Key to start.
        key: GameKey,
        /// Canonical exe path.
        executable_path: PathBuf,
        /// Best-effort display name derived from the window metadata.
        display_name: String,
    },
    /// Emit `Stopped` for the previous foreground, then `Started` for
    /// a different accepted one.
    Switch {
        /// Key to stop.
        stop: GameKey,
        /// Key to start.
        start: GameKey,
        /// Canonical exe path of the new foreground.
        executable_path: PathBuf,
        /// Best-effort display name.
        display_name: String,
    },
}

/// Compute the transition for a foreground-change event.
///
/// * `current` is the source's currently-tracked foreground (if any).
/// * `meta` describes the new foreground window.
/// * `decision` is the gate's verdict for the new window's PID.
#[must_use]
pub(super) fn transition_for(
    current: Option<&AcceptedForeground>,
    meta: &ForegroundMeta,
    decision: GateDecision,
) -> Transition {
    // Fast path: activation for the same PID we're already tracking is
    // a no-op. Happens a lot on alt-tab round-trips.
    if current.is_some_and(|c| c.pid == meta.pid) {
        return Transition::None;
    }

    match decision {
        GateDecision::Accept(accepted) => {
            let key = native_key(&accepted);
            let display_name = display_name_from(meta, &accepted);
            match current {
                Some(prev) => Transition::Switch {
                    stop: prev.key.clone(),
                    start: key,
                    executable_path: accepted.executable_path,
                    display_name,
                },
                None => Transition::Start {
                    key,
                    executable_path: accepted.executable_path,
                    display_name,
                },
            }
        }
        GateDecision::Reject(_) => match current {
            Some(prev) => Transition::Stop {
                key: prev.key.clone(),
            },
            None => Transition::None,
        },
    }
}

fn native_key(accepted: &AcceptedProcess) -> GameKey {
    GameKey::native(accepted.executable_path.to_string_lossy().into_owned())
}

fn display_name_from(meta: &ForegroundMeta, accepted: &AcceptedProcess) -> String {
    // Preference order: window resource class → window caption →
    // executable file stem → executable full path. The enrichment
    // cascade will refine this on a background task.
    if !meta.resource_class.trim().is_empty() {
        return meta.resource_class.clone();
    }
    if !meta.caption.trim().is_empty() {
        return meta.caption.clone();
    }
    accepted
        .executable_path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map_or_else(
            || accepted.executable_path.to_string_lossy().into_owned(),
            str::to_owned,
        )
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

    fn foreground(pid: u32, path: &str) -> AcceptedForeground {
        AcceptedForeground {
            pid,
            key: GameKey::native(path),
            executable_path: PathBuf::from(path),
        }
    }

    #[test]
    fn accept_with_nothing_current_starts() {
        let t = transition_for(
            None,
            &meta(100, "Celeste", "Celeste"),
            accepted("/opt/celeste/Celeste"),
        );
        match t {
            Transition::Start {
                key,
                executable_path,
                display_name,
            } => {
                assert_eq!(key, GameKey::native("/opt/celeste/Celeste"));
                assert_eq!(executable_path, PathBuf::from("/opt/celeste/Celeste"));
                assert_eq!(display_name, "Celeste");
            }
            other => panic!("expected Start, got {other:?}"),
        }
    }

    #[test]
    fn accept_different_pid_switches() {
        let prev = foreground(100, "/opt/celeste/Celeste");
        let t = transition_for(
            Some(&prev),
            &meta(200, "NewGame", ""),
            accepted("/opt/newgame/newgame"),
        );
        match t {
            Transition::Switch {
                stop,
                start,
                display_name,
                ..
            } => {
                assert_eq!(stop, GameKey::native("/opt/celeste/Celeste"));
                assert_eq!(start, GameKey::native("/opt/newgame/newgame"));
                assert_eq!(display_name, "NewGame");
            }
            other => panic!("expected Switch, got {other:?}"),
        }
    }

    #[test]
    fn accept_same_pid_is_noop() {
        let prev = foreground(100, "/opt/celeste/Celeste");
        let t = transition_for(
            Some(&prev),
            &meta(100, "Celeste", "Celeste — Chapter 1"),
            accepted("/opt/celeste/Celeste"),
        );
        assert_eq!(t, Transition::None);
    }

    #[test]
    fn reject_with_nothing_current_is_noop() {
        let t = transition_for(None, &meta(100, "Firefox", "ludex - GitHub"), rejected());
        assert_eq!(t, Transition::None);
    }

    #[test]
    fn reject_stops_previous() {
        let prev = foreground(100, "/opt/celeste/Celeste");
        let t = transition_for(Some(&prev), &meta(200, "Firefox", ""), rejected());
        assert_eq!(
            t,
            Transition::Stop {
                key: GameKey::native("/opt/celeste/Celeste"),
            }
        );
    }

    #[test]
    fn display_name_falls_back_to_caption_then_exe() {
        // Empty resource_class → use caption.
        let t = transition_for(
            None,
            &meta(100, "", "Some Window Title"),
            accepted("/opt/foo/foo"),
        );
        let Transition::Start { display_name, .. } = t else {
            panic!("expected Start");
        };
        assert_eq!(display_name, "Some Window Title");

        // Both empty → use exe stem.
        let t = transition_for(None, &meta(100, "", ""), accepted("/opt/foo/foo_bin"));
        let Transition::Start { display_name, .. } = t else {
            panic!("expected Start");
        };
        assert_eq!(display_name, "foo_bin");
    }
}
