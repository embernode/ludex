//! Non-launcher decision gate.
//!
//! Given a candidate PID and a foreground-window context, decide
//! whether this process should be tracked as a game. Stateless —
//! callers manage the "already running" set.
//!
//! The decision is split into a pure [`decide_from_inputs`] function
//! and an async [`Gate::decide`] wrapper that reads `/proc` and feeds
//! the pure function. All observation happens in the wrapper; the
//! pure function is thoroughly unit-testable without `/proc` at all.
//!
//! ## The heuristic
//!
//! A process is accepted as a game when **all** of the following hold:
//!
//! 1. Its executable is not in the user blocklist. Compositor,
//!    shell, and launcher binaries are the baseline blocklist.
//! 2. Its `maps` contains at least one graphics library
//!    ([`GraphicsLibraries::any`]). Ordinary desktop apps link Qt or
//!    GTK, not raw GL/Vulkan/SDL, so this is a strong positive signal.
//! 3. Either its window is fullscreen on its output, **or** the
//!    process is using a meaningful amount of GPU memory (crossing
//!    [`GateConfig::gpu_memory_threshold_bytes`]).
//!
//! Anything else is rejected with a typed reason suitable for logs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::proc::{environ, exe, fdinfo, maps};

/// Knobs the daemon supplies to the gate.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Executable file names (basename only) that should never be
    /// treated as games — shell, compositor, launcher binaries, and
    /// anything the user has added to their `blocked_applications`
    /// table.
    pub blocklist: HashSet<String>,

    /// Minimum per-process GPU memory in bytes required to accept a
    /// non-fullscreen window. Games use gigabytes; desktop apps use
    /// kilobytes. 50 MiB is a conservative separator.
    pub gpu_memory_threshold_bytes: u64,

    /// Environment-variable names that mark a process as launched by a
    /// known launcher. A process whose `/proc/<pid>/environ` contains
    /// any of these is handled by the corresponding launcher source
    /// (Steam / Lutris / Heroic), not by the foreground fallback.
    /// Without this list the daemon double-counts every Proton game:
    /// once via `SteamSource.content_log`, once via
    /// `KWinForegroundSource` on the Wine process's foreground window.
    pub launcher_env_vars: HashSet<String>,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            blocklist: default_blocklist(),
            gpu_memory_threshold_bytes: 50 * 1024 * 1024,
            launcher_env_vars: default_launcher_env_vars(),
        }
    }
}

/// Environment variables that mean "a launcher started this process,
/// so don't count it again from the foreground source".
///
/// Kept to vars that are only set on the *game's* own process
/// (not inherited by unrelated children of the user shell): Steam's
/// `SteamGameId` / `SteamAppId`, Proton's `STEAM_COMPAT_APP_ID`,
/// Lutris's `LUTRIS_GAME_UUID`, Heroic's `HEROIC_APP_NAME`.
///
/// Inherited-wrapper variables (`STEAM_RUNTIME`, `STEAM_BASE_FOLDER`,
/// `PRESSURE_VESSEL_*`) are deliberately **not** included: they show
/// up in every Steam-originating child shell too and would cause
/// false rejections.
#[must_use]
pub fn default_launcher_env_vars() -> HashSet<String> {
    [
        "SteamGameId",
        "SteamAppId",
        "STEAM_COMPAT_APP_ID",
        "LUTRIS_GAME_UUID",
        "HEROIC_APP_NAME",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// Baseline never-track binaries on KDE Plasma.
///
/// The list is intentionally conservative — things that *always* load
/// a graphics library, have *always* a high GPU footprint, and are
/// *never* games. Everything more contentious (browsers, video
/// players, creative apps) is left to the user to blocklist
/// case-by-case.
#[must_use]
pub fn default_blocklist() -> HashSet<String> {
    [
        // KDE Plasma shell / compositor stack.
        "kwin_wayland",
        "kwin_x11",
        "plasmashell",
        "kded5",
        "kded6",
        "krunner",
        "kactivitymanagerd",
        "kwalletd5",
        "kwalletd6",
        "ksmserver",
        "systemsettings",
        "systemsettings5",
        "polkit-kde-authentication-agent-1",
        // X server / Xwayland.
        "Xwayland",
        "Xorg",
        // GNOME (best-effort; ludex targets Plasma but costs nothing
        // to cover).
        "gnome-shell",
        "mutter",
        "gdm-x-session",
        // Ludex itself.
        "ludex-daemon",
        "ludex",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// Fields the daemon collects about the foreground window/process and
/// passes into the gate.
#[derive(Debug, Clone, Copy)]
pub struct GateInput {
    /// Process id of the candidate.
    pub pid: u32,
    /// Whether the candidate's window is fullscreen on its output.
    pub window_is_fullscreen: bool,
}

/// The gate's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Track this PID.
    Accept(AcceptedProcess),
    /// Don't track, with the reason the gate said no.
    Reject(RejectionReason),
}

/// Information about a process the gate accepted — everything needed
/// to construct a `NewApplication` and open a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedProcess {
    /// Resolved `/proc/<pid>/exe`.
    pub executable_path: PathBuf,
    /// Which graphics stacks the process has mapped in.
    pub graphics_libraries: maps::GraphicsLibraries,
}

/// Why the gate rejected a PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionReason {
    /// `/proc/<pid>/exe` could not be read (dead process, restricted).
    ExeUnreadable,
    /// The executable's basename is in the blocklist.
    Blocklisted,
    /// The process's environment contains a launcher-attribution
    /// variable; the launcher's own source (Steam/Lutris/Heroic)
    /// handles it instead of the fallback.
    AttributedToLauncher,
    /// `/proc/<pid>/maps` could not be read.
    MapsUnreadable,
    /// No graphics library was mapped into the process.
    NoGraphicsLibrary,
    /// Window is not fullscreen and the process's GPU memory footprint
    /// is below threshold.
    NotFullscreenAndLowGpu,
}

/// Pure decision logic.
///
/// Every input it depends on is explicit. No `/proc` reads, no async,
/// no hidden state — which makes every decision branch straightforward
/// to unit-test. Kept crate-internal; callers outside the daemon go
/// through [`Gate::decide`].
#[must_use]
pub(crate) fn decide_from_inputs(
    exe: Option<&Path>,
    environ: Option<&HashMap<String, String>>,
    libs: Option<maps::GraphicsLibraries>,
    gpu: Option<&fdinfo::GpuSummary>,
    window_is_fullscreen: bool,
    config: &GateConfig,
) -> GateDecision {
    let Some(exe_path) = exe else {
        return GateDecision::Reject(RejectionReason::ExeUnreadable);
    };
    if is_blocklisted(exe_path, &config.blocklist) {
        return GateDecision::Reject(RejectionReason::Blocklisted);
    }
    if let Some(env) = environ {
        if env.keys().any(|k| config.launcher_env_vars.contains(k)) {
            return GateDecision::Reject(RejectionReason::AttributedToLauncher);
        }
    }
    let Some(libs) = libs else {
        return GateDecision::Reject(RejectionReason::MapsUnreadable);
    };
    if !libs.any() {
        return GateDecision::Reject(RejectionReason::NoGraphicsLibrary);
    }
    if !window_is_fullscreen {
        let memory = gpu.map_or(0, |g| g.memory_bytes);
        if memory < config.gpu_memory_threshold_bytes {
            return GateDecision::Reject(RejectionReason::NotFullscreenAndLowGpu);
        }
    }
    GateDecision::Accept(AcceptedProcess {
        executable_path: exe_path.to_path_buf(),
        graphics_libraries: libs,
    })
}

fn is_blocklisted(exe: &Path, blocklist: &HashSet<String>) -> bool {
    let Some(name) = exe.file_name().and_then(std::ffi::OsStr::to_str) else {
        return false;
    };
    blocklist.contains(name)
}

/// Stateless gate instance. Constructed once and reused for every
/// foreground-window event.
#[derive(Debug, Clone)]
pub struct Gate {
    config: GateConfig,
}

impl Gate {
    /// Construct a gate with the given configuration.
    #[must_use]
    pub const fn new(config: GateConfig) -> Self {
        Self { config }
    }

    /// Read `/proc/<pid>/*` and compute a decision.
    ///
    /// Reads run in short-circuit order: exe, maps, and fdinfo only
    /// when needed. A process that dies mid-evaluation yields
    /// `RejectionReason::ExeUnreadable` or `MapsUnreadable` rather
    /// than propagating the I/O error.
    pub async fn decide(&self, input: GateInput) -> GateDecision {
        let Ok(exe_path) = exe::read(input.pid).await else {
            return GateDecision::Reject(RejectionReason::ExeUnreadable);
        };
        if is_blocklisted(&exe_path, &self.config.blocklist) {
            return GateDecision::Reject(RejectionReason::Blocklisted);
        }
        // Launcher-attribution check. Reading environ is cheap (single
        // file read) and short-circuits the more expensive maps/fdinfo
        // reads when the process is already owned by a launcher.
        let env = environ::read(input.pid).await.ok();
        if let Some(env) = env.as_ref() {
            if env
                .keys()
                .any(|k| self.config.launcher_env_vars.contains(k))
            {
                return GateDecision::Reject(RejectionReason::AttributedToLauncher);
            }
        }
        let Ok(libs) = maps::read(input.pid).await else {
            return GateDecision::Reject(RejectionReason::MapsUnreadable);
        };
        if !libs.any() {
            return GateDecision::Reject(RejectionReason::NoGraphicsLibrary);
        }
        // Skip the expensive fdinfo walk entirely when the window is
        // already fullscreen; we'd accept either way.
        let gpu = if input.window_is_fullscreen {
            None
        } else {
            fdinfo::read(input.pid).await.ok()
        };
        decide_from_inputs(
            Some(&exe_path),
            env.as_ref(),
            Some(libs),
            gpu.as_ref(),
            input.window_is_fullscreen,
            &self.config,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::fdinfo::GpuSummary;
    use crate::proc::maps::GraphicsLibraries;

    fn cfg() -> GateConfig {
        GateConfig {
            blocklist: ["kwin_wayland".to_owned(), "plasmashell".to_owned()]
                .into_iter()
                .collect(),
            gpu_memory_threshold_bytes: 10 * 1024 * 1024,
            launcher_env_vars: default_launcher_env_vars(),
        }
    }

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn gl_only() -> GraphicsLibraries {
        GraphicsLibraries {
            opengl: true,
            ..Default::default()
        }
    }

    fn no_libs() -> GraphicsLibraries {
        GraphicsLibraries::default()
    }

    #[test]
    fn missing_exe_rejects() {
        let d = decide_from_inputs(None, None, Some(gl_only()), None, true, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::ExeUnreadable));
    }

    #[test]
    fn blocklisted_exe_rejects_even_if_fullscreen_and_gl() {
        let exe = PathBuf::from("/usr/bin/kwin_wayland");
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), None, true, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::Blocklisted));
    }

    #[test]
    fn missing_maps_rejects() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, None, None, true, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::MapsUnreadable));
    }

    #[test]
    fn no_graphics_library_rejects() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, Some(no_libs()), None, true, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::NoGraphicsLibrary));
    }

    #[test]
    fn fullscreen_with_graphics_library_accepts() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), None, true, &cfg());
        match d {
            GateDecision::Accept(a) => {
                assert_eq!(a.executable_path, exe);
                assert!(a.graphics_libraries.opengl);
            }
            GateDecision::Reject(_) => panic!("expected accept"),
        }
    }

    #[test]
    fn non_fullscreen_without_gpu_rejects() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), None, false, &cfg());
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::NotFullscreenAndLowGpu)
        );
    }

    #[test]
    fn non_fullscreen_below_threshold_rejects() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let gpu = GpuSummary {
            driver: Some("amdgpu".into()),
            memory_bytes: 5 * 1024 * 1024, // below 10 MiB threshold
            engine_nanoseconds: 0,
        };
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), Some(&gpu), false, &cfg());
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::NotFullscreenAndLowGpu)
        );
    }

    #[test]
    fn non_fullscreen_over_threshold_accepts() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let gpu = GpuSummary {
            driver: Some("amdgpu".into()),
            memory_bytes: 500 * 1024 * 1024, // 500 MiB, well above
            engine_nanoseconds: 123,
        };
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), Some(&gpu), false, &cfg());
        assert!(matches!(d, GateDecision::Accept(_)));
    }

    #[test]
    fn steam_env_rejects_with_launcher_attribution() {
        let exe = PathBuf::from("/home/u/.steam/steamapps/common/foo/foo.exe");
        let env = env_of(&[("SteamGameId", "440"), ("SteamAppId", "440")]);
        // Fullscreen + graphics library would otherwise accept; the
        // environ check must short-circuit first.
        let d = decide_from_inputs(Some(&exe), Some(&env), Some(gl_only()), None, true, &cfg());
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
    }

    #[test]
    fn proton_compat_env_rejects() {
        let exe = PathBuf::from("/home/u/game.exe");
        let env = env_of(&[("STEAM_COMPAT_APP_ID", "730")]);
        let d = decide_from_inputs(Some(&exe), Some(&env), Some(gl_only()), None, true, &cfg());
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
    }

    #[test]
    fn lutris_uuid_env_rejects() {
        let exe = PathBuf::from("/home/u/games/foo");
        let env = env_of(&[("LUTRIS_GAME_UUID", "abc-123")]);
        let d = decide_from_inputs(Some(&exe), Some(&env), Some(gl_only()), None, true, &cfg());
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
    }

    #[test]
    fn heroic_env_rejects() {
        let exe = PathBuf::from("/home/u/Games/legendary/foo/foo.exe");
        let env = env_of(&[("HEROIC_APP_NAME", "com.example.foo")]);
        let d = decide_from_inputs(Some(&exe), Some(&env), Some(gl_only()), None, true, &cfg());
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
    }

    #[test]
    fn generic_steam_runtime_env_does_not_reject() {
        // Variables inherited by every child of a terminal that had
        // Steam started from it must not trigger a rejection —
        // otherwise `ludex-daemon` itself (or a user shell descendant)
        // could be wrongly rejected.
        let exe = PathBuf::from("/opt/games/foo/foo");
        let env = env_of(&[
            (
                "STEAM_RUNTIME",
                "/home/u/.local/share/Steam/ubuntu12_32/steam-runtime",
            ),
            ("STEAM_BASE_FOLDER", "/home/u/.local/share/Steam"),
        ]);
        let d = decide_from_inputs(Some(&exe), Some(&env), Some(gl_only()), None, true, &cfg());
        assert!(matches!(d, GateDecision::Accept(_)), "got {d:?}");
    }

    #[test]
    fn default_launcher_env_vars_covers_expected_names() {
        let vars = default_launcher_env_vars();
        for expected in [
            "SteamGameId",
            "SteamAppId",
            "STEAM_COMPAT_APP_ID",
            "LUTRIS_GAME_UUID",
            "HEROIC_APP_NAME",
        ] {
            assert!(vars.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn default_blocklist_includes_kwin_and_plasmashell() {
        let bl = default_blocklist();
        for expected in ["kwin_wayland", "kwin_x11", "plasmashell", "ludex-daemon"] {
            assert!(bl.contains(expected), "blocklist missing {expected}");
        }
    }

    #[tokio::test]
    async fn async_decide_rejects_self_for_no_graphics_library() {
        // The test binary does not link GL/Vulkan/SDL, so the gate
        // should reject with NoGraphicsLibrary. This exercises the
        // real /proc read path.
        let gate = Gate::new(GateConfig::default());
        let decision = gate
            .decide(GateInput {
                pid: std::process::id(),
                window_is_fullscreen: true,
            })
            .await;
        // `ludex-daemon` is in the default blocklist, but the *test*
        // binary is named after the integration-test file, not the
        // daemon, so we'd land on NoGraphicsLibrary.
        match decision {
            GateDecision::Reject(
                RejectionReason::NoGraphicsLibrary | RejectionReason::Blocklisted,
            ) => {}
            other => panic!("expected Reject(NoGraphicsLibrary | Blocklisted); got {other:?}"),
        }
    }
}
