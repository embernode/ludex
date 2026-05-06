//! Heroic Games Launcher store-cache enricher.
//!
//! Heroic is a third-party launcher for Epic, GOG, and Amazon Prime Gaming
//! libraries. Like Lutris, it doesn't expose a lifecycle signal we can
//! subscribe to (no `Started` / `Stopped` D-Bus event, no log we can tail
//! cleanly across versions), so the daemon's foreground-window source
//! picks Heroic-launched games up the same way it picks up bare native
//! binaries. Heroic-launched processes do inherit `HEROIC_APP_NAME`,
//! which the gate intentionally does *not* treat as a rejection signal —
//! see the doc comment on `default_launcher_env_vars` in the daemon's
//! gate module.
//!
//! Match shape:
//!
//! 1. Read the three runner-specific library caches — Legendary (Epic),
//!    GOG, and Nile (Amazon) — from `$HOME/.config/heroic/store_cache/`.
//!    Each is a JSON document whose top-level wrapper is either
//!    `{"library": [...]}` (Legendary, Nile) or `{"games": [...]}` (GOG)
//!    around an array of games.
//! 2. Filter to entries with `is_installed == true` and a usable
//!    `install.install_path`; uninstalled / DLC-only rows have a stub
//!    `install` object and would otherwise pollute the prefix-match.
//! 3. Pick the matching entry. The strategy depends on the application's
//!    `launcher_type`:
//!    - `Heroic`: the gate has already keyed the row by Heroic's own
//!      app_name (Epic GUID / GOG product id / Amazon ASIN), so do a
//!      direct lookup by `app_name == launcher_id`. This survives the
//!      wine-variant-switching that Heroic exposes per game — the
//!      executable path the daemon captured points at the wine
//!      preloader and varies per variant.
//!    - `Native`: legacy fallback for processes that somehow reached
//!      the gate without `HEROIC_APP_NAME` (e.g., a user-curated
//!      desktop shortcut bypassing Heroic). Find the entry whose
//!      `install.install_path` is the longest path-component prefix
//!      of the candidate's executable.
//! 4. Surface `title` as `product_name`, `developer` (when present) as
//!    `publisher`, and the joined `install_path + install.executable`
//!    as `executable_path` so the database row reads as the real
//!    Windows .exe rather than the wine preloader.
//!
//! The library caches are re-read on every enrich call rather than
//! cached. Enrichment runs once per newly-discovered application — not
//! per session start — so the cost (a one-shot O(N) parse of files
//! totalling under a few megabytes) is paid rarely and avoids holding a
//! second copy of the user's library list in daemon memory.

use std::path::{Path, PathBuf};

use ludex_core::{Application, IdentityUpdate, LauncherType};
use serde::Deserialize;
use tracing::{debug, warn};

use crate::context::EnrichmentContext;

/// Names of the per-runner library cache files under
/// `<heroic_config>/store_cache/`. Order matters only for the warn log
/// in case of malformed JSON; matching is across the union.
const LIBRARY_FILES: &[&str] = &[
    "legendary_library.json",
    "gog_library.json",
    "nile_library.json",
];

/// Heroic library top-level wrapper. Legendary and Nile use
/// `{"library": [...]}`; GOG uses `{"games": [...]}`. `alias` lets one
/// struct deserialize both shapes.
#[derive(Debug, Deserialize)]
struct LibraryFile {
    #[serde(alias = "games")]
    library: Vec<GameEntry>,
}

/// A single entry from a Heroic library cache, narrowed to the fields
/// this enricher reads. Heroic carries dozens of fields per game (cover
/// art URLs, save-folder hints, store URL, etc.); leaving them out of
/// the struct means a future Heroic version adding more is a no-op for us.
#[derive(Debug, Deserialize, Clone)]
struct GameEntry {
    app_name: Option<String>,
    title: Option<String>,
    developer: Option<String>,
    #[serde(default)]
    is_installed: bool,
    install: Option<InstallInfo>,
}

/// The `install` sub-object carries divergent shapes — for installed
/// games it has `install_path` and `executable`; for DLCs and
/// uninstalled rows it can be `{"is_dlc": true}` or empty `{}`. All
/// fields are optional so deserialization doesn't fail on those rows.
#[derive(Debug, Deserialize, Clone)]
struct InstallInfo {
    install_path: Option<String>,
    executable: Option<String>,
}

/// Internal flat representation: a known-installed Heroic game with the
/// fields needed to match either by app_name (Heroic-keyed
/// applications) or by install_path prefix (Native fallback).
#[derive(Debug, Clone)]
struct HeroicGame {
    app_name: String,
    title: String,
    developer: Option<String>,
    install_path: String,
    /// Game's `.exe` basename relative to `install_path`. Surfaced as
    /// `executable_path` so the database stores the real Windows
    /// binary instead of the wine preloader.
    executable: Option<String>,
}

/// Enrich an application from Heroic's runner-specific library caches.
pub async fn enrich(app: &Application, ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    let heroic_dir = ctx.heroic_config_dir.as_ref()?;

    let games = read_libraries(heroic_dir).await;
    if games.is_empty() {
        return None;
    }
    let matched = match app.launcher_type {
        LauncherType::Heroic => find_by_app_name(&games, &app.launcher_id)?,
        LauncherType::Native => {
            let exe = app.executable_path.as_ref()?;
            find_by_install_path(&games, Path::new(exe))?
        }
        // Other launcher types have their own authoritative enrichers;
        // don't second-guess them by smuggling in Heroic data.
        _ => return None,
    };

    debug!(
        launcher_type = ?app.launcher_type,
        launcher_id = %app.launcher_id,
        heroic_title = %matched.title,
        "heroic enricher matched",
    );
    Some(IdentityUpdate {
        product_name: Some(matched.title.clone()),
        publisher: matched.developer.clone(),
        executable_path: real_executable_path(matched),
        ..Default::default()
    })
}

/// Concatenate `install_path` and `executable` into the absolute path
/// of the game's actual `.exe`. Returns `None` if either piece is
/// missing — falling back to the wine preloader the daemon captured
/// is no improvement over what the row already holds.
fn real_executable_path(game: &HeroicGame) -> Option<String> {
    let exe = game.executable.as_deref()?.trim();
    if exe.is_empty() {
        return None;
    }
    Some(
        PathBuf::from(&game.install_path)
            .join(exe)
            .to_string_lossy()
            .into_owned(),
    )
}

/// Read every available runner's library cache and concatenate the
/// installed games. A missing or malformed file produces a warn log and
/// is skipped so one runner's bad state can't block matches from the
/// others.
async fn read_libraries(heroic_dir: &Path) -> Vec<HeroicGame> {
    let cache_dir = heroic_dir.join("store_cache");
    let mut all = Vec::new();
    for filename in LIBRARY_FILES {
        let path = cache_dir.join(filename);
        match read_one_library(&path).await {
            Ok(games) => all.extend(games),
            Err(LibraryReadError::Missing) => {} // first-run / runner not used
            Err(LibraryReadError::Io(e)) => {
                warn!(path = %path.display(), error = %e, "could not read Heroic library cache");
            }
            Err(LibraryReadError::Parse(e)) => {
                warn!(path = %path.display(), error = %e, "could not parse Heroic library cache");
            }
        }
    }
    all
}

#[derive(Debug)]
enum LibraryReadError {
    Missing,
    Io(std::io::Error),
    Parse(serde_json::Error),
}

async fn read_one_library(path: &Path) -> Result<Vec<HeroicGame>, LibraryReadError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LibraryReadError::Missing);
        }
        Err(e) => return Err(LibraryReadError::Io(e)),
    };
    let file: LibraryFile = serde_json::from_slice(&bytes).map_err(LibraryReadError::Parse)?;
    Ok(file
        .library
        .into_iter()
        .filter_map(|g| {
            if !g.is_installed {
                return None;
            }
            let app_name = g.app_name.filter(|s| !s.trim().is_empty())?;
            let install = g.install?;
            let install_path = install.install_path.filter(|s| !s.trim().is_empty())?;
            let title = g.title.filter(|s| !s.trim().is_empty())?;
            Some(HeroicGame {
                app_name,
                title,
                developer: g.developer.filter(|d| !d.trim().is_empty()),
                install_path,
                executable: install.executable.filter(|s| !s.trim().is_empty()),
            })
        })
        .collect())
}

/// Direct lookup by Heroic's own canonical id. Used when the
/// application row was opened by the foreground source as
/// `LauncherType::Heroic` (the launcher_id is the `HEROIC_APP_NAME`
/// the gate captured from the process environ).
fn find_by_app_name<'a>(games: &'a [HeroicGame], app_name: &str) -> Option<&'a HeroicGame> {
    games.iter().find(|g| g.app_name == app_name)
}

/// Fallback for `LauncherType::Native` candidates: longest
/// path-component prefix match against `install_path`. Path-component
/// prefix — not byte-prefix — so `/foo/bar` does not match
/// `/foo/barbaz`.
fn find_by_install_path<'a>(games: &'a [HeroicGame], exe_path: &Path) -> Option<&'a HeroicGame> {
    games
        .iter()
        .filter(|g| {
            let dir = PathBuf::from(&g.install_path);
            exe_path.starts_with(&dir)
        })
        .max_by_key(|g| g.install_path.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        app_name: &str,
        title: &str,
        install_path: &str,
        executable: Option<&str>,
        developer: Option<&str>,
    ) -> HeroicGame {
        HeroicGame {
            app_name: app_name.to_owned(),
            title: title.to_owned(),
            developer: developer.map(str::to_owned),
            install_path: install_path.to_owned(),
            executable: executable.map(str::to_owned),
        }
    }

    #[test]
    fn find_by_app_name_returns_exact_match() {
        let g = vec![
            entry("aaa", "Foo", "/g/foo", Some("Foo.exe"), None),
            entry("bbb", "Bar", "/g/bar", Some("Bar.exe"), None),
        ];
        let m = find_by_app_name(&g, "bbb").expect("should match");
        assert_eq!(m.title, "Bar");
    }

    #[test]
    fn find_by_app_name_returns_none_when_unknown() {
        let g = vec![entry("aaa", "Foo", "/g/foo", None, None)];
        assert!(find_by_app_name(&g, "zzz").is_none());
    }

    #[test]
    fn find_by_install_path_picks_longest_prefix() {
        let g = vec![
            entry("a", "Outer", "/home/u/Games/Heroic/outer", None, None),
            entry("b", "Inner", "/home/u/Games/Heroic/outer/inner", None, None),
        ];
        let exe = PathBuf::from("/home/u/Games/Heroic/outer/inner/game.exe");
        let m = find_by_install_path(&g, &exe).expect("should match");
        assert_eq!(m.title, "Inner");
    }

    #[test]
    fn find_by_install_path_does_not_match_byte_prefix_only() {
        let g = vec![entry(
            "a",
            "Sibling",
            "/home/u/Games/Heroic/foo",
            None,
            None,
        )];
        let exe = PathBuf::from("/home/u/Games/Heroic/foobar/game.exe");
        assert!(find_by_install_path(&g, &exe).is_none());
    }

    #[test]
    fn real_executable_path_joins_install_path_and_executable() {
        let g = entry(
            "a",
            "Doors - Paradox",
            "/home/u/Games/Heroic/Doors",
            Some("Doors Paradox.exe"),
            None,
        );
        assert_eq!(
            real_executable_path(&g).as_deref(),
            Some("/home/u/Games/Heroic/Doors/Doors Paradox.exe"),
        );
    }

    #[test]
    fn real_executable_path_none_when_executable_missing() {
        let g = entry("a", "Foo", "/g/foo", None, None);
        assert!(real_executable_path(&g).is_none());
    }

    #[tokio::test]
    async fn read_one_library_filters_uninstalled_and_dlc_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("legendary_library.json");
        // Three rows: a normal installed game, an uninstalled game,
        // and a DLC-shaped row whose `install` only carries `is_dlc`.
        // Only the first should round-trip.
        let json = r#"{
            "library": [
                {
                    "app_name": "abc",
                    "title": "Doors - Paradox",
                    "developer": "Big Loop Studios",
                    "is_installed": true,
                    "install": {
                        "executable": "Doors Paradox.exe",
                        "install_path": "/home/u/Games/Heroic/Doors"
                    }
                },
                {
                    "app_name": "def",
                    "title": "Uninstalled Title",
                    "is_installed": false,
                    "install": {}
                },
                {
                    "app_name": "ghi",
                    "title": "Some DLC",
                    "is_installed": true,
                    "install": {"is_dlc": true}
                }
            ]
        }"#;
        tokio::fs::write(&path, json).await.unwrap();
        let games = read_one_library(&path).await.unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].app_name, "abc");
        assert_eq!(games[0].title, "Doors - Paradox");
        assert_eq!(games[0].developer.as_deref(), Some("Big Loop Studios"));
        assert_eq!(games[0].install_path, "/home/u/Games/Heroic/Doors");
        assert_eq!(games[0].executable.as_deref(), Some("Doors Paradox.exe"));
    }

    #[tokio::test]
    async fn read_one_library_accepts_gog_games_alias() {
        // GOG wraps under `games` rather than `library`; the same
        // struct handles both via `#[serde(alias)]`.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gog_library.json");
        let json = r#"{
            "games": [
                {
                    "app_name": "1207666373",
                    "title": "A Short Hike",
                    "is_installed": true,
                    "install": {"install_path": "/home/u/Games/Heroic/A Short Hike"}
                }
            ]
        }"#;
        tokio::fs::write(&path, json).await.unwrap();
        let games = read_one_library(&path).await.unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title, "A Short Hike");
    }

    #[tokio::test]
    async fn read_libraries_combines_runners_and_skips_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("store_cache");
        tokio::fs::create_dir_all(&cache).await.unwrap();
        tokio::fs::write(
            cache.join("legendary_library.json"),
            r#"{"library":[{"app_name":"a","title":"Epic Game","is_installed":true,"install":{"install_path":"/g/epic"}}]}"#,
        ).await.unwrap();
        tokio::fs::write(
            cache.join("gog_library.json"),
            r#"{"games":[{"app_name":"b","title":"GOG Game","is_installed":true,"install":{"install_path":"/g/gog"}}]}"#,
        ).await.unwrap();

        let games = read_libraries(tmp.path()).await;
        let titles: Vec<_> = games.iter().map(|g| g.title.as_str()).collect();
        assert!(titles.contains(&"Epic Game"));
        assert!(titles.contains(&"GOG Game"));
        assert_eq!(games.len(), 2);
    }

    #[tokio::test]
    async fn read_one_library_missing_file_is_distinct_error() {
        let tmp = tempfile::tempdir().unwrap();
        let result = read_one_library(&tmp.path().join("nope.json")).await;
        assert!(matches!(result, Err(LibraryReadError::Missing)));
    }
}
