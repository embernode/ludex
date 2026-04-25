//! Lutris `pga.db` enricher.
//!
//! Many users install non-Steam games — Battle.net, EA App, Epic, plain
//! wine titles — through Lutris, which keeps a SQLite database at
//! `$XDG_DATA_HOME/lutris/pga.db` listing every installed game with its
//! display name and install directory. Lutris itself doesn't expose a
//! lifecycle signal we can subscribe to (start/stop), so the daemon's
//! foreground-window source picks these games up the same way it picks
//! up bare native binaries — the catch is that Lutris-launched
//! processes inherit `LUTRIS_GAME_UUID`, which used to short-circuit
//! the gate's launcher-attribution check. The matching gate change
//! removes that variable from the rejection set so this enricher
//! actually has something to enrich.
//!
//! Match shape:
//!
//! 1. Read every installed Lutris game's `name` + `directory`.
//! 2. Find the game whose `directory` is the longest path-component
//!    prefix of the candidate's executable. Path-component prefix —
//!    not byte-prefix — so `/home/u/Games/foo` does not match
//!    `/home/u/Games/foobar/...`.
//! 3. If the matched row is one of the launcher-aggregator wine
//!    prefixes (Battle.net today; Epic / EA / Ubisoft are deliberately
//!    out of scope this commit), look the executable's basename up in a
//!    curated map. Battle.net's pga.db row is *the launcher itself* —
//!    Diablo IV, WoW, Overwatch, etc. don't get their own Lutris
//!    rows — so without curation every Blizzard game would just read
//!    "Battle.net". The curated map is small and only updated when
//!    Blizzard ships something new.
//! 4. Otherwise (a plain Lutris-installed wine game like a standalone
//!    indie title), the Lutris row's `name` is already what the user
//!    expects, so we use it verbatim.

use std::path::{Path, PathBuf};

use ludex_core::{Application, IdentityUpdate};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::ConnectOptions;
use tracing::{debug, warn};

use crate::context::EnrichmentContext;

/// One row from Lutris's `games` table, narrowed to the columns this
/// enricher actually reads. The schema is stable enough across recent
/// Lutris versions that pinning to these names is safe.
#[derive(Debug, Clone, sqlx::FromRow)]
struct LutrisGame {
    name: String,
    directory: String,
}

/// Enrich an application using Lutris's installed-games database.
pub async fn enrich(app: &Application, ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    let pga_db = ctx.lutris_pga_db.as_ref()?;
    let exe = app.executable_path.as_ref()?;
    let exe_path = Path::new(exe);

    let games = read_installed_games(pga_db).await.ok()?;
    let matched = best_match(&games, exe_path)?;

    let (product_name, publisher) = resolve_identity(&matched.name, exe_path);
    debug!(
        path = %exe_path.display(),
        lutris_name = %matched.name,
        %product_name,
        "lutris enricher matched",
    );
    Some(IdentityUpdate {
        product_name: Some(product_name),
        publisher,
        ..Default::default()
    })
}

/// Open the pga.db read-only and return every row with a usable
/// `directory`. Lutris keeps a few not-yet-installed and uninstalled
/// rows around; we filter to `installed = 1` to avoid matching a
/// deleted game's stale path.
async fn read_installed_games(pga_db: &Path) -> sqlx::Result<Vec<LutrisGame>> {
    // Read-only open: prevents an enricher bug from corrupting the
    // user's Lutris library, and avoids contending with Lutris's
    // own writes for the WAL lock.
    let opts = SqliteConnectOptions::new().filename(pga_db).read_only(true);
    let mut conn = match opts.connect().await {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %pga_db.display(), error = %e, "could not open Lutris pga.db");
            return Err(e);
        }
    };
    let rows: Vec<LutrisGame> = sqlx::query_as::<_, LutrisGame>(
        "SELECT name, directory FROM games \
         WHERE installed = 1 AND name IS NOT NULL AND directory IS NOT NULL \
            AND directory != ''",
    )
    .fetch_all(&mut conn)
    .await?;
    Ok(rows)
}

/// Return the row whose `directory` is the longest path-component
/// prefix of `exe_path`. `None` if no row's directory matches.
fn best_match<'a>(games: &'a [LutrisGame], exe_path: &Path) -> Option<&'a LutrisGame> {
    games
        .iter()
        .filter(|g| {
            let dir = PathBuf::from(&g.directory);
            exe_path.starts_with(&dir)
        })
        .max_by_key(|g| g.directory.len())
}

/// Map a Lutris row's `name` plus the candidate's basename to the
/// final (`product_name`, optional `publisher`) pair. The launcher-
/// aggregator special case is keyed on the Lutris row name so adding
/// Epic / EA / Ubisoft later is a one-table addition.
fn resolve_identity(lutris_name: &str, exe_path: &Path) -> (String, Option<String>) {
    if lutris_name.eq_ignore_ascii_case("Battle.net") {
        if let Some(basename) = exe_path.file_name().and_then(|s| s.to_str()) {
            if let Some(curated) = curated_battlenet_game(basename) {
                return (curated.to_owned(), Some("Blizzard Entertainment".to_owned()));
            }
        }
        // Fall through: we matched the Battle.net wine prefix but the
        // exe basename isn't in the curated table. Returning the
        // Lutris row's name ("Battle.net") is at least correct, just
        // less specific.
    }
    (lutris_name.to_owned(), None)
}

/// Map a Battle.net game's executable basename to its display name.
/// The set is small and stable — Blizzard's catalogue grows by maybe
/// one or two titles a year — so a hard-coded match is preferable
/// to a config file the user has to maintain.
///
/// Match is case-insensitive on the basename (some Wine builds case-
/// fold differently than others). The function returns the canonical
/// English title; future i18n would key locale on a separate field.
fn curated_battlenet_game(basename: &str) -> Option<&'static str> {
    let key = basename.to_ascii_lowercase();
    Some(match key.as_str() {
        "wow.exe" | "wowclassic.exe" => "World of Warcraft",
        "diablo iv.exe" => "Diablo IV",
        "diablo iii.exe" | "diablo iii64.exe" => "Diablo III",
        "diablo ii resurrected.exe" => "Diablo II: Resurrected",
        "overwatch.exe" => "Overwatch 2",
        "hearthstone.exe" => "Hearthstone",
        "starcraft.exe" => "StarCraft: Remastered",
        "starcraft ii.exe" | "sc2.exe" => "StarCraft II",
        "heroes of the storm.exe" | "heroesoftheswarm.exe" => "Heroes of the Storm",
        "warcraft iii.exe" | "wc3.exe" => "Warcraft III: Reforged",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn games(rows: &[(&str, &str)]) -> Vec<LutrisGame> {
        rows.iter()
            .map(|(name, dir)| LutrisGame {
                name: (*name).to_owned(),
                directory: (*dir).to_owned(),
            })
            .collect()
    }

    #[test]
    fn best_match_picks_longest_prefix() {
        let g = games(&[
            ("Battle.net", "/home/u/Games/battlenet"),
            ("Lone Wolf", "/home/u/Games/lone-wolf"),
        ]);
        let exe = PathBuf::from(
            "/home/u/Games/battlenet/drive_c/Program Files (x86)/Diablo IV/Diablo IV.exe",
        );
        let m = best_match(&g, &exe).expect("should match");
        assert_eq!(m.name, "Battle.net");
    }

    #[test]
    fn best_match_returns_none_for_unrelated_path() {
        let g = games(&[("Battle.net", "/home/u/Games/battlenet")]);
        let exe = PathBuf::from("/home/u/Steam/steamapps/common/Game/game.exe");
        assert!(best_match(&g, &exe).is_none());
    }

    #[test]
    fn best_match_does_not_match_byte_prefix_only() {
        // The two directories share a byte prefix but are different
        // paths. The longer one must not be considered a "match" of
        // the shorter via byte arithmetic.
        let g = games(&[("Sibling", "/home/u/Games/foo")]);
        let exe = PathBuf::from("/home/u/Games/foobar/game.exe");
        assert!(best_match(&g, &exe).is_none());
    }

    #[test]
    fn best_match_picks_longer_when_a_subdir_was_also_installed() {
        // Hypothetical: a user installed a wine game inside the
        // Battle.net wine prefix (rare, but possible). The deeper
        // directory should win so the dedicated row's name is used.
        let g = games(&[
            ("Battle.net", "/home/u/Games/battlenet"),
            (
                "Some Standalone Game",
                "/home/u/Games/battlenet/standalone",
            ),
        ]);
        let exe = PathBuf::from("/home/u/Games/battlenet/standalone/Game.exe");
        let m = best_match(&g, &exe).expect("should match");
        assert_eq!(m.name, "Some Standalone Game");
    }

    #[test]
    fn battlenet_curation_resolves_known_titles() {
        let exe =
            PathBuf::from("/home/u/Games/battlenet/drive_c/.../Diablo IV/Diablo IV.exe");
        let (name, publisher) = resolve_identity("Battle.net", &exe);
        assert_eq!(name, "Diablo IV");
        assert_eq!(publisher.as_deref(), Some("Blizzard Entertainment"));
    }

    #[test]
    fn battlenet_curation_falls_back_when_basename_unknown() {
        let exe = PathBuf::from("/home/u/Games/battlenet/drive_c/.../Mystery/Mystery.exe");
        let (name, publisher) = resolve_identity("Battle.net", &exe);
        assert_eq!(name, "Battle.net");
        assert!(publisher.is_none());
    }

    #[test]
    fn non_aggregator_lutris_name_is_used_verbatim() {
        let exe = PathBuf::from("/home/u/Games/some-indie/game.exe");
        let (name, publisher) = resolve_identity("Some Indie Game", &exe);
        assert_eq!(name, "Some Indie Game");
        assert!(publisher.is_none());
    }

    #[test]
    fn battlenet_curation_is_case_insensitive_on_basename() {
        let exe = PathBuf::from("/home/u/Games/battlenet/drive_c/.../Wow/WOW.EXE");
        let (name, _) = resolve_identity("Battle.net", &exe);
        assert_eq!(name, "World of Warcraft");
    }

    #[test]
    fn battlenet_match_is_case_insensitive_on_lutris_name() {
        // Some Lutris installs name the row "battle.net" lowercased.
        let exe = PathBuf::from("/home/u/Games/battlenet/drive_c/.../Wow/Wow.exe");
        let (name, _) = resolve_identity("battle.net", &exe);
        assert_eq!(name, "World of Warcraft");
    }

    #[tokio::test]
    async fn read_installed_games_excludes_uninstalled_rows() {
        // Build a tiny pga.db-shaped database in-memory and round-trip
        // through `read_installed_games` to confirm the WHERE clause
        // does what we think it does. Uses an actual SQLite file so
        // the SQL string is exercised end-to-end, not just the Rust
        // type system.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("pga.db");
        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let mut conn = opts.connect().await.unwrap();
        sqlx::query(
            "CREATE TABLE games (\
                id INTEGER PRIMARY KEY, name TEXT, directory TEXT, installed INTEGER)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO games(name, directory, installed) VALUES (?, ?, 1)")
            .bind("Installed Game")
            .bind("/home/u/Games/installed")
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO games(name, directory, installed) VALUES (?, ?, 0)")
            .bind("Uninstalled Game")
            .bind("/home/u/Games/uninstalled")
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO games(name, directory, installed) VALUES (?, NULL, 1)")
            .bind("Missing Directory")
            .execute(&mut conn)
            .await
            .unwrap();
        drop(conn);

        let rows = read_installed_games(&db_path).await.unwrap();
        assert_eq!(rows.len(), 1, "only the installed row with a directory should be returned");
        assert_eq!(rows[0].name, "Installed Game");
    }
}
