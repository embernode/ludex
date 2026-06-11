//! Shared configuration for enrichment sources.
//!
//! Resolving every filesystem location once at daemon start means the
//! per-source code stays dumb: each source receives a path or `None`
//! and does not touch the environment itself. Tests construct the
//! context explicitly to point at temporary fixtures.

use std::path::{Path, PathBuf};

/// Paths consulted by the enrichment sources.
#[derive(Debug, Clone, Default)]
pub struct EnrichmentContext {
    /// XDG application directories to scan for `.desktop` files. Ordered
    /// from user-writable to system-wide; later entries do **not**
    /// overwrite names found earlier.
    pub desktop_dirs: Vec<PathBuf>,

    /// Steam data directory, if Steam is installed (for
    /// `steamapps/appmanifest_*.acf` lookups). `None` if not found.
    pub steam_dir: Option<PathBuf>,

    /// Path to the Lutris SQLite database, if Lutris is installed.
    /// Consumed by the Lutris enrichment source for product-name
    /// lookups by install-directory prefix.
    pub lutris_pga_db: Option<PathBuf>,

    /// Heroic configuration directory, if Heroic is installed.
    /// Consumed by the Heroic enrichment source for
    /// `store_cache/*_library.json` title lookups.
    pub heroic_config_dir: Option<PathBuf>,
}

impl EnrichmentContext {
    /// Detect every path from the process environment.
    ///
    /// - `desktop_dirs` are populated from `$XDG_DATA_DIRS` plus the
    ///   conventional user and Flatpak locations.
    /// - `steam_dir`, `lutris_pga_db`, `heroic_config_dir` default to
    ///   their conventional locations under `$XDG_DATA_HOME` / `$HOME`;
    ///   absent files fall out to `None`.
    #[must_use]
    pub fn detect_from_env() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg_data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));

        let desktop_dirs = detect_desktop_dirs(home.as_deref(), xdg_data_home.as_deref());
        let steam_dir = xdg_data_home
            .as_ref()
            .map(|x| x.join("Steam"))
            .filter(|p| p.is_dir());
        let heroic_config_dir = home
            .as_ref()
            .map(|h| h.join(".config/heroic"))
            .filter(|p| p.is_dir());
        let lutris_pga_db = xdg_data_home
            .as_ref()
            .map(|x| x.join("lutris/pga.db"))
            .filter(|p| p.is_file());

        Self {
            desktop_dirs,
            steam_dir,
            lutris_pga_db,
            heroic_config_dir,
        }
    }
}

fn detect_desktop_dirs(home: Option<&Path>, xdg_data_home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(xdh) = xdg_data_home {
        dirs.push(xdh.join("applications"));
        // Flatpak user exports.
        dirs.push(xdh.join("flatpak/exports/share/applications"));
    }

    // System + user overrides from XDG_DATA_DIRS.
    if let Some(v) = std::env::var_os("XDG_DATA_DIRS") {
        for part in std::env::split_paths(&v) {
            let candidate = part.join("applications");
            if !dirs.contains(&candidate) {
                dirs.push(candidate);
            }
        }
    }

    // Flatpak system exports.
    let flatpak_system = PathBuf::from("/var/lib/flatpak/exports/share/applications");
    if !dirs.contains(&flatpak_system) {
        dirs.push(flatpak_system);
    }

    // Conventional fallbacks in case XDG_DATA_DIRS is unset.
    for fallback in ["/usr/local/share/applications", "/usr/share/applications"] {
        let p = PathBuf::from(fallback);
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }

    // Home desktop (unusual but possible).
    if let Some(h) = home {
        let p = h.join("Desktop");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }

    dirs
}
