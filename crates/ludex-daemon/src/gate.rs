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

use crate::config::SharedConfig;
use crate::proc::{environ, exe, fdinfo, maps, tree};

/// `/proc/<pid>/comm` values of the gamescope nested compositor. A
/// process whose ancestry contains one of these is trusted as a game
/// without the usual fullscreen / graphics-library gating — gamescope
/// only hosts games, and the gating heuristics were designed for
/// direct KWin-managed windows, not nested ones.
///
/// In practice this ancestry check seldom fires on the KWin foreground
/// path. When a game runs under nested gamescope, gamescope owns the
/// Wayland surface KWin reports, so the foreground PID *is* gamescope —
/// an ancestry match needs a descendant PID, which KWin never hands us.
/// Such games are still tracked: they present fullscreen, so the
/// fullscreen accept path covers them, and launcher-attributed titles
/// (Heroic/Lutris) inherit their id env var into gamescope and name
/// correctly via the enricher. The lone gap is a *native* game run
/// under gamescope, recorded under the `gamescope` binary — a rare edge
/// case, intentionally not special-cased (descending gamescope's
/// subtree to find the real game is fragile, and yields only the wine
/// preloader for the common Proton case). See `docs/architecture.md`.
const GAMESCOPE_COMMS: &[&str] = &["gamescope", "gamescope-wl"];

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
    /// launcher with its own authoritative lifecycle source (today:
    /// Steam, via `SteamSource.content_log`). A process whose
    /// `/proc/<pid>/environ` carries any of these with a *nonzero* appid
    /// value is handled by the corresponding launcher source, not by the
    /// foreground fallback — without this list the daemon double-counts
    /// every Proton game. Matched by value (not presence) so non-Steam
    /// shortcuts, which set a zero appid, still reach the foreground
    /// source — see [`attributed_to_steam`].
    pub launcher_env_vars: HashSet<String>,

    /// Environment-variable names that identify a process as launched
    /// by a *foreground-source* launcher — Lutris and Heroic, which
    /// have no lifecycle signal of their own and so are picked up
    /// solely through the foreground-window source. Presence of any of
    /// these *overrides* `launcher_env_vars`: Heroic-via-Proton and
    /// Lutris-via-Proton both transitively inherit Steam's
    /// `STEAM_COMPAT_APP_ID`, but Steam itself never saw the launch
    /// and won't fire a content_log entry, so honouring the
    /// Steam-attribution rejection in that case would silently drop
    /// every Proton-running Heroic / Lutris game.
    pub foreground_source_launcher_env_vars: HashSet<String>,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            blocklist: default_blocklist(),
            gpu_memory_threshold_bytes: 50 * 1024 * 1024,
            launcher_env_vars: default_launcher_env_vars(),
            foreground_source_launcher_env_vars: default_foreground_source_launcher_env_vars(),
        }
    }
}

/// Environment variables that mean "Steam started this process, so
/// don't count it again from the foreground source — Steam's content
/// log will report it."
///
/// Kept to the *appid* vars set on the game's own process: Steam's
/// `SteamAppId` and Proton's `STEAM_COMPAT_APP_ID`. Both are matched by
/// **value**, not mere presence — a real Steam app carries a nonzero
/// appid, whereas a non-Steam game added to Steam as a shortcut carries
/// the same variable names with a zero appid. Steam never depot-tracks
/// shortcuts (no `appmanifest`, no content-log line), so rejecting them
/// here would leave them tracked by nobody; the value check lets them
/// fall through to the foreground source. See [`attributed_to_steam`].
///
/// `SteamGameId` is deliberately excluded: it holds a nonzero synthetic
/// id for shortcuts too, so it cannot tell a real app from a shortcut,
/// and a real Steam launch always also sets `SteamAppId`.
///
/// Inherited-wrapper variables (`STEAM_RUNTIME`, `STEAM_BASE_FOLDER`,
/// `PRESSURE_VESSEL_*`) are deliberately **not** included: they show
/// up in every Steam-originating child shell too and would cause
/// false rejections.
///
/// `LUTRIS_GAME_UUID` and `HEROIC_APP_NAME` belong to a separate
/// category (see `default_foreground_source_launcher_env_vars`):
/// they identify launchers with no authoritative source, and their
/// presence overrides the rejection here when both are set on the
/// same process — a Heroic-via-Proton or Lutris-via-Proton launch
/// transitively inherits `STEAM_COMPAT_APP_ID` even though Steam
/// itself never saw it.
#[must_use]
pub fn default_launcher_env_vars() -> HashSet<String> {
    ["SteamAppId", "STEAM_COMPAT_APP_ID"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Environment variables identifying processes launched by a
/// foreground-source launcher (Lutris, Heroic). Their presence on a
/// process overrides any concurrent Steam-attribution variables in
/// `default_launcher_env_vars` — a Heroic-launched game running
/// through Proton inherits `STEAM_COMPAT_APP_ID` as a side effect of
/// Proton's setup, but Steam itself never tracked it, so the
/// foreground source must remain the path that picks it up.
#[must_use]
pub fn default_foreground_source_launcher_env_vars() -> HashSet<String> {
    ["LUTRIS_GAME_UUID", "HEROIC_APP_NAME"]
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

/// Returns `true` when any ancestor process of `pid` reports a
/// [`GAMESCOPE_COMMS`] value in `/proc/<ppid>/comm`. Used by the gate
/// to bypass fullscreen / graphics-library gating for windows that
/// are really rendered inside a nested gamescope compositor.
fn has_gamescope_ancestor(pid: u32) -> bool {
    tree::ancestors(pid).any(|p| tree::comm(p).is_ok_and(|c| GAMESCOPE_COMMS.contains(&c.as_str())))
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
    /// Foreground-source launcher attribution, if the gate detected
    /// one in the process environ. Used by the kwin-source's
    /// transition module to construct a stable [`GameKey`] keyed off
    /// the launcher's own canonical id (e.g. Heroic's app_name)
    /// rather than the wine-preloader path that `executable_path`
    /// holds for Heroic-via-Proton games.
    pub attribution: Option<LauncherAttribution>,
}

/// Identifies the launcher that produced an accepted process when the
/// process belongs to a foreground-source launcher (one without its
/// own lifecycle source). The carried id is the launcher's own
/// invariant identifier — survives wine-version changes, install-path
/// moves, and library refreshes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherAttribution {
    /// Process inherited Heroic Games Launcher's `HEROIC_APP_NAME`.
    /// The contained id is the runner-specific canonical key — Epic
    /// GUID, GOG product id, or Amazon ASIN depending on which
    /// runner Heroic invoked for the game.
    Heroic {
        /// Value of `HEROIC_APP_NAME` from the process environ.
        app_name: String,
    },
    /// Process carries Lutris's `LUTRIS_GAME_UUID`. Lutris has no
    /// lifecycle source of its own, so the foreground-window source
    /// tracks it; keying by this UUID rather than the wine-preloader
    /// exe path (shared by every Lutris game on a runner) keeps each
    /// game on its own application row.
    Lutris {
        /// Value of `LUTRIS_GAME_UUID` from the process environ.
        uuid: String,
    },
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
///
/// `gamescope_ancestry` signals that the process runs inside a nested
/// gamescope compositor. When true, the fullscreen/GPU gate is
/// bypassed and a missing `maps` or graphics-library read degrades to
/// accept rather than reject — gamescope only hosts games, and its
/// nested-window presentation can hide signals the outer compositor
/// would otherwise see.
#[must_use]
pub(crate) fn decide_from_inputs(
    exe: Option<&Path>,
    environ: Option<&HashMap<String, String>>,
    libs: Option<maps::GraphicsLibraries>,
    gpu: Option<&fdinfo::GpuSummary>,
    window_is_fullscreen: bool,
    gamescope_ancestry: bool,
    config: &GateConfig,
) -> GateDecision {
    let Some(exe_path) = exe else {
        return GateDecision::Reject(RejectionReason::ExeUnreadable);
    };
    if is_blocklisted(exe_path, &config.blocklist) {
        return GateDecision::Reject(RejectionReason::Blocklisted);
    }
    if let Some(env) = environ {
        let foreground_attributed = env
            .keys()
            .any(|k| config.foreground_source_launcher_env_vars.contains(k));
        if !foreground_attributed && attributed_to_steam(env, config) {
            return GateDecision::Reject(RejectionReason::AttributedToLauncher);
        }
    }
    let libs = match libs {
        Some(l) => l,
        None if gamescope_ancestry => maps::GraphicsLibraries::default(),
        None => return GateDecision::Reject(RejectionReason::MapsUnreadable),
    };
    if !libs.any() && !gamescope_ancestry {
        return GateDecision::Reject(RejectionReason::NoGraphicsLibrary);
    }
    if !(window_is_fullscreen || gamescope_ancestry) {
        let memory = gpu.map_or(0, |g| g.memory_bytes);
        if memory < config.gpu_memory_threshold_bytes {
            return GateDecision::Reject(RejectionReason::NotFullscreenAndLowGpu);
        }
    }
    GateDecision::Accept(AcceptedProcess {
        executable_path: exe_path.to_path_buf(),
        graphics_libraries: libs,
        attribution: environ.and_then(extract_launcher_attribution),
    })
}

/// Extract a foreground-source launcher's canonical id from the
/// process environ. Returns `None` if no recognised attribution
/// variable is present.
fn extract_launcher_attribution(env: &HashMap<String, String>) -> Option<LauncherAttribution> {
    if let Some(name) = env.get("HEROIC_APP_NAME") {
        let name = name.trim();
        if !name.is_empty() {
            return Some(LauncherAttribution::Heroic {
                app_name: name.to_owned(),
            });
        }
    }
    if let Some(uuid) = env.get("LUTRIS_GAME_UUID") {
        let uuid = uuid.trim();
        if !uuid.is_empty() {
            return Some(LauncherAttribution::Lutris {
                uuid: uuid.to_owned(),
            });
        }
    }
    None
}

/// Whether `env` attributes the process to a *real* Steam app — one the
/// Steam content-log source will track — as opposed to a non-Steam game
/// added to Steam as a shortcut.
///
/// A launcher env var (`SteamAppId`, `STEAM_COMPAT_APP_ID`) counts only
/// when it carries a nonzero appid. Shortcuts carry the same names with
/// a zero appid and are never depot-tracked by Steam, so a zero value
/// means "not Steam-owned" and the process must fall through to the
/// foreground source rather than be rejected as `AttributedToLauncher`.
fn attributed_to_steam(env: &HashMap<String, String>, config: &GateConfig) -> bool {
    config.launcher_env_vars.iter().any(|key| {
        env.get(key)
            .and_then(|v| v.trim().parse::<u64>().ok())
            .is_some_and(|appid| appid != 0)
    })
}

/// Whether `env` carries any launcher-attribution variable at all — a
/// Steam appid var or a foreground-source launcher id (by presence, not
/// value). Used to decide whether a process's environ is the one that
/// holds the launch's attribution, or whether to keep looking up the
/// process tree.
fn has_launcher_attribution(env: &HashMap<String, String>, config: &GateConfig) -> bool {
    env.keys().any(|k| {
        config.launcher_env_vars.contains(k)
            || config.foreground_source_launcher_env_vars.contains(k)
    })
}

/// Resolve the environ that carries a window's launcher attribution.
///
/// A game process can re-exec itself with the Steam/launcher env vars
/// stripped from its *own* environ — shapez.io does exactly this: the
/// `shapezio` window process shows no `SteamAppId`, while its ancestors
/// in the pressure-vessel launch chain (`bash` → `pv-adverb` →
/// `reaper`) still carry `SteamAppId=<appid>`. Reading only the window
/// process then mis-classifies the game as a native window and tracks
/// it a second time alongside the authoritative Steam source.
///
/// So when the window's `own` environ shows no attribution, fall back to
/// the nearest ancestor environ that does. `ancestors` yields ancestor
/// environs nearest-first (parent, grandparent, …). If neither the
/// window nor any ancestor carries a launcher var, `own` is returned
/// unchanged and the process flows through to the normal game gate.
fn resolve_attribution_environ(
    own: HashMap<String, String>,
    ancestors: impl IntoIterator<Item = HashMap<String, String>>,
    config: &GateConfig,
) -> HashMap<String, String> {
    if has_launcher_attribution(&own, config) {
        return own;
    }
    ancestors
        .into_iter()
        .find(|env| has_launcher_attribution(env, config))
        .unwrap_or(own)
}

fn is_blocklisted(exe: &Path, blocklist: &HashSet<String>) -> bool {
    let Some(name) = exe.file_name().and_then(std::ffi::OsStr::to_str) else {
        return false;
    };
    blocklist.contains(name)
}

/// Stateless gate instance. Constructed once and reused for every
/// foreground-window event. Holds a shared-config handle so the D-
/// Bus setters can swap values in without tearing the gate down.
#[derive(Debug, Clone)]
pub struct Gate {
    config: SharedConfig,
}

impl Gate {
    /// Construct a gate bound to the shared tracker configuration.
    /// Read access in [`Gate::decide`] takes a read guard on the
    /// underlying `RwLock`; writers are rare (user-driven settings
    /// changes) so there is no meaningful contention.
    #[must_use]
    pub const fn new(config: SharedConfig) -> Self {
        Self { config }
    }

    /// Read `/proc/<pid>/*` and compute a decision.
    ///
    /// Reads run in short-circuit order: exe, maps, and fdinfo only
    /// when needed. A process that dies mid-evaluation yields
    /// `RejectionReason::ExeUnreadable` or `MapsUnreadable` rather
    /// than propagating the I/O error.
    pub async fn decide(&self, input: GateInput) -> GateDecision {
        // Snapshot the gate fields up front so we hold the read
        // guard only for the pointer-copy, not across the `/proc`
        // reads below. A writer calling `SetGpuMemoryThresholdBytes`
        // between this snapshot and the final decision just lands
        // the new value on the next activation — acceptable, and it
        // means the lock never fights the proc syscalls.
        let config = self.config.read().await.gate.clone();
        let Ok(exe_path) = exe::read(input.pid).await else {
            return GateDecision::Reject(RejectionReason::ExeUnreadable);
        };
        if is_blocklisted(&exe_path, &config.blocklist) {
            return GateDecision::Reject(RejectionReason::Blocklisted);
        }
        // Launcher-attribution check. Reading environ is cheap (single
        // file read) and short-circuits the more expensive maps/fdinfo
        // reads when the process is already owned by a launcher with
        // its own lifecycle source. A foreground-source launcher
        // (Heroic, Lutris) overrides the rejection — Heroic-via-Proton
        // and Lutris-via-Proton inherit `STEAM_COMPAT_APP_ID` even
        // though Steam itself never saw the launch.
        let env = match environ::read(input.pid).await.ok() {
            // No environ (process gone or restricted): nothing to attribute.
            None => None,
            // The window process carries its own attribution — the common
            // case (native Steam games, Proton, Heroic, Lutris). Use it
            // directly, no ancestor walk needed.
            Some(own) if has_launcher_attribution(&own, &config) => Some(own),
            // The window process shows no attribution. It may have
            // re-exec'd itself with the launcher vars stripped (shapez.io
            // does this) while an ancestor in the launch chain kept them.
            // Walk up until an ancestor carries attribution, then decide
            // against that environ so the game isn't counted a second time
            // as a native window alongside the Steam source.
            //
            // This reads each ancestor's environ, but only for a window
            // whose own environ lacks attribution — most non-games find
            // nothing and fall through. Foreground activations are
            // user-paced and the gate already walks the ancestor chain for
            // the gamescope check below, so the extra reads are negligible.
            Some(own) => {
                let mut ancestors = Vec::new();
                for anc in tree::ancestors(input.pid) {
                    if let Ok(anc_env) = environ::read(anc).await {
                        let attributed = has_launcher_attribution(&anc_env, &config);
                        ancestors.push(anc_env);
                        if attributed {
                            break;
                        }
                    }
                }
                Some(resolve_attribution_environ(own, ancestors, &config))
            }
        };
        if let Some(env) = env.as_ref() {
            let foreground_attributed = env
                .keys()
                .any(|k| config.foreground_source_launcher_env_vars.contains(k));
            if !foreground_attributed && attributed_to_steam(env, &config) {
                return GateDecision::Reject(RejectionReason::AttributedToLauncher);
            }
        }
        let gamescope_ancestry = has_gamescope_ancestor(input.pid);
        let libs = maps::read(input.pid).await.ok();
        // Skip the expensive fdinfo walk when the fullscreen or
        // gamescope shortcut already covers us.
        let gpu = if input.window_is_fullscreen || gamescope_ancestry {
            None
        } else {
            fdinfo::read(input.pid).await.ok()
        };
        decide_from_inputs(
            Some(&exe_path),
            env.as_ref(),
            libs,
            gpu.as_ref(),
            input.window_is_fullscreen,
            gamescope_ancestry,
            &config,
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
            foreground_source_launcher_env_vars: default_foreground_source_launcher_env_vars(),
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
    fn has_launcher_attribution_detects_steam_and_foreground_vars() {
        let c = cfg();
        assert!(has_launcher_attribution(
            &env_of(&[("SteamAppId", "1318690")]),
            &c
        ));
        assert!(has_launcher_attribution(
            &env_of(&[("STEAM_COMPAT_APP_ID", "1318690")]),
            &c
        ));
        assert!(has_launcher_attribution(
            &env_of(&[("HEROIC_APP_NAME", "x")]),
            &c
        ));
        assert!(has_launcher_attribution(
            &env_of(&[("LUTRIS_GAME_UUID", "u")]),
            &c
        ));
        assert!(!has_launcher_attribution(
            &env_of(&[("PATH", "/usr/bin")]),
            &c
        ));
        assert!(!has_launcher_attribution(&HashMap::new(), &c));
    }

    /// shapez.io: the window process (`own`) and its immediate shapezio
    /// parent have the Steam vars stripped; the bash launch wrapper two
    /// levels up still carries them. Resolution must recover them.
    #[test]
    fn resolve_falls_back_to_ancestor_that_carries_the_appid() {
        let own = HashMap::new();
        let ancestors = vec![
            env_of(&[("PWD", "/game")]), // shapezio parent: stripped
            env_of(&[
                ("SteamAppId", "1318690"),
                ("STEAM_COMPAT_APP_ID", "1318690"),
            ]), // bash wrapper
        ];
        let resolved = resolve_attribution_environ(own, ancestors, &cfg());
        assert_eq!(
            resolved.get("SteamAppId").map(String::as_str),
            Some("1318690")
        );
    }

    /// Native Steam / Proton games carry the appid on the window process
    /// itself — use it directly and never consult ancestors (an ancestor
    /// could belong to a different, outer launch).
    #[test]
    fn resolve_prefers_own_environ_when_it_has_attribution() {
        let own = env_of(&[("SteamAppId", "440")]);
        let ancestors = vec![env_of(&[("SteamAppId", "999")])];
        let resolved = resolve_attribution_environ(own, ancestors, &cfg());
        assert_eq!(resolved.get("SteamAppId").map(String::as_str), Some("440"));
    }

    /// A genuine native (non-launcher) game: neither the window nor any
    /// ancestor carries a launcher var, so the environ is unchanged and
    /// the process flows through to the fullscreen/GPU gate.
    #[test]
    fn resolve_returns_own_when_no_ancestor_has_attribution() {
        let own = env_of(&[("HOME", "/home/x")]);
        let ancestors = vec![env_of(&[("PATH", "/usr/bin")]), HashMap::new()];
        let resolved = resolve_attribution_environ(own.clone(), ancestors, &cfg());
        assert_eq!(resolved, own);
    }

    /// End-to-end: a `shapezio` window whose own environ is bare but
    /// whose launch chain carries the appid must be rejected as already
    /// tracked by the Steam source — not accepted as a native game. This
    /// is the fix for the native+Steam double-count.
    #[test]
    fn shapezio_window_rejected_via_ancestor_appid() {
        let c = cfg();
        let exe = PathBuf::from("/home/u/.local/share/Steam/steamapps/common/shapez.io/shapezio");
        let resolved = resolve_attribution_environ(
            HashMap::new(),
            vec![env_of(&[
                ("SteamAppId", "1318690"),
                ("STEAM_COMPAT_APP_ID", "1318690"),
            ])],
            &c,
        );
        let d = decide_from_inputs(
            Some(&exe),
            Some(&resolved),
            Some(gl_only()),
            None,
            true,
            false,
            &c,
        );
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
        // Guard: without the ancestor resolution (bare own environ), the
        // same window would be accepted — the bug we're fixing.
        let bare = HashMap::new();
        let d_bug = decide_from_inputs(
            Some(&exe),
            Some(&bare),
            Some(gl_only()),
            None,
            true,
            false,
            &c,
        );
        assert!(matches!(d_bug, GateDecision::Accept(_)));
    }

    #[test]
    fn missing_exe_rejects() {
        let d = decide_from_inputs(None, None, Some(gl_only()), None, true, false, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::ExeUnreadable));
    }

    #[test]
    fn blocklisted_exe_rejects_even_if_fullscreen_and_gl() {
        let exe = PathBuf::from("/usr/bin/kwin_wayland");
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), None, true, false, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::Blocklisted));
    }

    #[test]
    fn missing_maps_rejects() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, None, None, true, false, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::MapsUnreadable));
    }

    #[test]
    fn no_graphics_library_rejects() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, Some(no_libs()), None, true, false, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::NoGraphicsLibrary));
    }

    #[test]
    fn fullscreen_with_graphics_library_accepts() {
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), None, true, false, &cfg());
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
        let d = decide_from_inputs(
            Some(&exe),
            None,
            Some(gl_only()),
            None,
            false,
            false,
            &cfg(),
        );
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
        let d = decide_from_inputs(
            Some(&exe),
            None,
            Some(gl_only()),
            Some(&gpu),
            false,
            false,
            &cfg(),
        );
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
        let d = decide_from_inputs(
            Some(&exe),
            None,
            Some(gl_only()),
            Some(&gpu),
            false,
            false,
            &cfg(),
        );
        assert!(matches!(d, GateDecision::Accept(_)));
    }

    #[test]
    fn steam_env_rejects_with_launcher_attribution() {
        let exe = PathBuf::from("/home/u/.steam/steamapps/common/foo/foo.exe");
        let env = env_of(&[("SteamGameId", "440"), ("SteamAppId", "440")]);
        // Fullscreen + graphics library would otherwise accept; the
        // environ check must short-circuit first.
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
    }

    #[test]
    fn proton_compat_env_rejects() {
        let exe = PathBuf::from("/home/u/game.exe");
        let env = env_of(&[("STEAM_COMPAT_APP_ID", "730")]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
    }

    #[test]
    fn lutris_uuid_env_does_not_reject() {
        // Lutris doesn't expose a lifecycle signal (no Started/Stopped
        // counterpart to the Steam ACF source), so the foreground-
        // window source is the only path that can pick up
        // Lutris-launched games. Rejecting on `LUTRIS_GAME_UUID`
        // would drop every one of them; the Lutris pga.db enricher
        // is what fills in the proper name afterwards.
        let exe = PathBuf::from("/home/u/games/foo");
        let env = env_of(&[("LUTRIS_GAME_UUID", "abc-123")]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert!(
            matches!(d, GateDecision::Accept(_)),
            "Lutris-attributed games must pass the gate; got {d:?}",
        );
    }

    #[test]
    fn lutris_uuid_env_produces_lutris_attribution() {
        // Every Lutris/bare-Wine game shares the same wine-preloader
        // exe path, so `executable_path` alone can't key sessions per
        // game (GATE-2). The gate must surface `LUTRIS_GAME_UUID` as a
        // `LauncherAttribution::Lutris` so the transition module can
        // key by it instead.
        let exe = PathBuf::from(
            "/home/u/.local/share/lutris/runners/wine/lutris-fshack/bin/wine64-preloader",
        );
        let env = env_of(&[("LUTRIS_GAME_UUID", "abc-123")]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        match d {
            GateDecision::Accept(accepted) => {
                assert_eq!(
                    accepted.attribution,
                    Some(LauncherAttribution::Lutris {
                        uuid: "abc-123".to_owned()
                    })
                );
            }
            GateDecision::Reject(_) => panic!("expected accept, got {d:?}"),
        }
    }

    #[test]
    fn heroic_app_name_env_does_not_reject() {
        // Heroic, like Lutris, has no lifecycle source — it's the
        // foreground-window source plus the Heroic store-cache enricher
        // that pick these games up. Rejecting on `HEROIC_APP_NAME`
        // would drop every Heroic-launched game on the floor.
        let exe = PathBuf::from("/home/u/Games/Heroic/Foo/foo.exe");
        let env = env_of(&[("HEROIC_APP_NAME", "com.example.foo")]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert!(
            matches!(d, GateDecision::Accept(_)),
            "Heroic-attributed games must pass the gate; got {d:?}",
        );
    }

    #[test]
    fn heroic_via_proton_overrides_steam_attribution() {
        // Heroic supports Proton-GE as an alternative to Wine. When a
        // user picks Proton, the game process inherits Proton's
        // `STEAM_COMPAT_APP_ID` (typically "0" for non-Steam games)
        // even though Steam itself never saw the launch. The
        // foreground-source attribution from `HEROIC_APP_NAME` must
        // win, otherwise every Heroic-via-Proton launch is silently
        // dropped — a real bug observed against LEGO Builder's
        // Journey on a live system.
        let exe = PathBuf::from("/home/u/Games/Heroic/LBJ/Builder.exe");
        let env = env_of(&[
            ("HEROIC_APP_NAME", "com.epicgames.lego.bj"),
            ("STEAM_COMPAT_APP_ID", "0"),
        ]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert!(
            matches!(d, GateDecision::Accept(_)),
            "Heroic-via-Proton games must pass the gate; got {d:?}",
        );
    }

    #[test]
    fn lutris_via_proton_overrides_steam_attribution() {
        // Same reasoning as the Heroic case: Lutris's wine/Proton
        // wrapper inherits `STEAM_COMPAT_APP_ID` for Proton runners,
        // but Steam never saw the launch, so the foreground source
        // (and the Lutris pga.db enricher) must remain authoritative.
        let exe = PathBuf::from("/home/u/Games/lutris-proton/game.exe");
        let env = env_of(&[
            ("LUTRIS_GAME_UUID", "abc-123"),
            ("STEAM_COMPAT_APP_ID", "0"),
        ]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert!(
            matches!(d, GateDecision::Accept(_)),
            "Lutris-via-Proton games must pass the gate; got {d:?}",
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
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert!(matches!(d, GateDecision::Accept(_)), "got {d:?}");
    }

    #[test]
    fn gamescope_ancestry_accepts_without_fullscreen_or_gpu() {
        // A game rendered inside gamescope may present as a
        // non-fullscreen window in the outer KWin, and GPU activity
        // gets attributed to the gamescope process, not the child.
        // Gamescope ancestry is itself the signal — accept outright.
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(
            Some(&exe),
            None,
            Some(gl_only()),
            None,
            /* window_is_fullscreen */ false,
            /* gamescope_ancestry */ true,
            &cfg(),
        );
        assert!(matches!(d, GateDecision::Accept(_)), "got {d:?}");
    }

    #[test]
    fn gamescope_ancestry_accepts_even_without_graphics_library() {
        // Gamescope children can be native Wayland processes whose
        // /proc/<pid>/maps we can read but that link no
        // libGL/libvulkan/libSDL (they talk directly to the nested
        // compositor via Wayland). Under gamescope, accept anyway.
        let exe = PathBuf::from("/opt/games/foo/foo");
        let d = decide_from_inputs(Some(&exe), None, Some(no_libs()), None, false, true, &cfg());
        assert!(matches!(d, GateDecision::Accept(_)), "got {d:?}");
    }

    #[test]
    fn gamescope_ancestry_does_not_override_blocklist() {
        // Even inside gamescope, the compositor binary itself (were it
        // somehow re-parented under a gamescope instance) must not be
        // tracked. Blocklist check fires first, before gamescope.
        let exe = PathBuf::from("/usr/bin/kwin_wayland");
        let d = decide_from_inputs(Some(&exe), None, Some(gl_only()), None, true, true, &cfg());
        assert_eq!(d, GateDecision::Reject(RejectionReason::Blocklisted));
    }

    #[test]
    fn gamescope_ancestry_does_not_override_launcher_attribution() {
        // A Steam game running inside gamescope is still owned by the
        // Steam source — the foreground fallback must reject to avoid
        // double-counting.
        let exe = PathBuf::from("/home/u/.steam/steamapps/common/foo/foo");
        let env = env_of(&[("SteamGameId", "440"), ("SteamAppId", "440")]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            true,
            &cfg(),
        );
        assert_eq!(
            d,
            GateDecision::Reject(RejectionReason::AttributedToLauncher)
        );
    }

    /// A non-Steam game added to Steam as a shortcut carries the Steam
    /// env var *names* but a zero appid (`SteamAppId=0`,
    /// `STEAM_COMPAT_APP_ID=0`) plus a nonzero synthetic `SteamGameId`.
    /// Steam never depot-tracks it, so the gate must accept it and let
    /// the foreground source record it — rejecting would leave it
    /// tracked by nobody (GATE-1).
    #[test]
    fn steam_shortcut_with_zero_appid_is_accepted() {
        let exe = PathBuf::from("/home/u/Games/foo/foo");
        let env = env_of(&[
            ("SteamGameId", "13275623096702517248"),
            ("SteamAppId", "0"),
            ("STEAM_COMPAT_APP_ID", "0"),
        ]);
        let d = decide_from_inputs(
            Some(&exe),
            Some(&env),
            Some(gl_only()),
            None,
            true,
            false,
            &cfg(),
        );
        assert!(
            matches!(d, GateDecision::Accept(_)),
            "Steam shortcut with zero appid must pass the gate; got {d:?}",
        );
    }

    #[test]
    fn default_launcher_env_vars_covers_expected_names() {
        let vars = default_launcher_env_vars();
        for expected in ["SteamAppId", "STEAM_COMPAT_APP_ID"] {
            assert!(vars.contains(expected), "missing {expected}");
        }
        // `SteamGameId` is excluded because it is nonzero even for
        // shortcuts; `LUTRIS_GAME_UUID` / `HEROIC_APP_NAME` are excluded
        // because they have no lifecycle source. See the doc comment on
        // `default_launcher_env_vars`.
        for excluded in ["SteamGameId", "LUTRIS_GAME_UUID", "HEROIC_APP_NAME"] {
            assert!(!vars.contains(excluded), "{excluded} must not gate-reject",);
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
        use crate::config::{BackupConfig, TrackerConfig};
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::sync::RwLock;
        let config = Arc::new(RwLock::new(TrackerConfig {
            gate: GateConfig::default(),
            alt_tab_grace: Duration::from_secs(15),
            pause_when_backgrounded: true,
            idle_grace: Duration::from_mins(5),
            backup: BackupConfig {
                interval: Duration::from_hours(24),
                retention: 14,
            },
        }));
        let gate = Gate::new(config);
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
