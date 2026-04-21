//! Steam launcher source.
//!
//! Detection strategy:
//!
//! 1. At startup, scan every `~/.local/share/Steam/steamapps/appmanifest_*.acf`
//!    file. Any manifest whose `StateFlags` bitmask has bit 64 set
//!    (`k_EAppStateAppRunning`) is a currently-running game; emit a
//!    [`Started`](GameEvent::Started) for each, with the product name read
//!    from the same manifest.
//! 2. Tail `~/.local/share/Steam/logs/content_log.txt`. When the file
//!    appends a line matching
//!    `[...] AppID <id> state changed : <flags>`, parse the flags. The
//!    presence of `App Running` transitions the appid into the "running"
//!    set (emit [`Started`]); its disappearance transitions out
//!    (emit [`Stopped`]).
//!
//! The parsers are deliberately small, total, and property-tested for
//! non-panic behaviour. Log rotation is handled by detecting truncation
//! (file length shorter than our read cursor) and re-opening.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use ludex_core::GameKey;
use notify::{recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use time::OffsetDateTime;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, instrument, warn};

use crate::event::GameEvent;

/// Steam `App Running` bit in the appmanifest `StateFlags` bitmask.
const APP_RUNNING_FLAG: u64 = 64;

/// Fallback poll cadence in case the filesystem watcher misses an event.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Launcher source that watches Steam's content log.
pub struct SteamSource {
    steam_dir: PathBuf,
}

impl SteamSource {
    /// Construct a source pointed at an explicit Steam data directory.
    #[must_use]
    pub fn new(steam_dir: PathBuf) -> Self {
        Self { steam_dir }
    }

    /// Look up the Steam data directory from the environment. Returns
    /// `None` if `HOME`/`XDG_DATA_HOME` is unset or the directory does not
    /// exist.
    #[must_use]
    pub fn detect_from_env() -> Option<Self> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        let candidate = base.join("Steam");
        if candidate.is_dir() {
            Some(Self::new(candidate))
        } else {
            None
        }
    }

    fn content_log_path(&self) -> PathBuf {
        self.steam_dir.join("logs/content_log.txt")
    }

    fn steamapps_dir(&self) -> PathBuf {
        self.steam_dir.join("steamapps")
    }

    /// Best-effort name lookup from `appmanifest_<appid>.acf`.
    async fn resolve_name(&self, appid: &str) -> String {
        let manifest = self
            .steamapps_dir()
            .join(format!("appmanifest_{appid}.acf"));
        match tokio::fs::read_to_string(&manifest).await {
            Ok(s) => {
                parse_vdf_top_level_string(&s, "name").unwrap_or_else(|| format!("AppID {appid}"))
            }
            Err(_) => format!("AppID {appid}"),
        }
    }

    /// Scan all installed appmanifests and emit [`GameEvent::Started`] for
    /// each whose `StateFlags` has the App Running bit set. Silently
    /// skips unreadable files.
    async fn cold_start_scan(
        &self,
        tx: &mpsc::Sender<GameEvent>,
        running: &mut HashSet<String>,
    ) -> Result<()> {
        let dir = self.steamapps_dir();
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(d) => d,
            Err(e) => {
                warn!(path = %dir.display(), error = %e, "steamapps dir unreadable; skipping cold-start scan");
                return Ok(());
            }
        };
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let Some(file_name) = name.to_str() else {
                continue;
            };
            let Some(appid) = file_name
                .strip_prefix("appmanifest_")
                .and_then(|s| s.strip_suffix(".acf"))
            else {
                continue;
            };
            if appid.is_empty() || !appid.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let content = match tokio::fs::read_to_string(entry.path()).await {
                Ok(c) => c,
                Err(e) => {
                    debug!(appid, error = %e, "skipping unreadable appmanifest");
                    continue;
                }
            };
            let flags = parse_vdf_top_level_u64(&content, "StateFlags").unwrap_or(0);
            if flags & APP_RUNNING_FLAG != 0 {
                let display_name = parse_vdf_top_level_string(&content, "name")
                    .unwrap_or_else(|| format!("AppID {appid}"));
                info!(appid, %display_name, "cold-start: detected running game");
                running.insert(appid.to_owned());
                if tx
                    .send(GameEvent::Started {
                        key: GameKey::steam(appid),
                        display_name,
                        at: OffsetDateTime::now_utc(),
                    })
                    .await
                    .is_err()
                {
                    // Receiver dropped; abort silently.
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Run the Steam source until `shutdown` fires.
    #[instrument(skip_all, fields(steam_dir = %self.steam_dir.display()))]
    pub async fn run(
        self,
        tx: mpsc::Sender<GameEvent>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut running: HashSet<String> = HashSet::new();

        // Subscribe first: the inotify watch is installed before the
        // cold-start scan so concurrent state changes are queued, not
        // lost.
        let log_path = self.content_log_path();
        let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<()>();
        // `Option` so we can keep the watcher alive for the life of this
        // task without needing to name its concrete type at the use site.
        let _watcher: Option<RecommendedWatcher> = if log_path.is_file() {
            match recommended_watcher(move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        let _ = notify_tx.send(());
                    }
                }
            }) {
                Ok(mut w) => match w.watch(&log_path, RecursiveMode::NonRecursive) {
                    Ok(()) => Some(w),
                    Err(e) => {
                        warn!(error = %e, "failed to install inotify watch; falling back to polling");
                        None
                    }
                },
                Err(e) => {
                    warn!(error = %e, "failed to construct inotify watcher; falling back to polling");
                    None
                }
            }
        } else {
            warn!(path = %log_path.display(), "content_log.txt not present; Steam source idle");
            None
        };

        self.cold_start_scan(&tx, &mut running).await?;

        // Open log + seek to end so only new lines are processed.
        let mut reader = match open_at_end(&log_path).await {
            Ok(r) => r,
            Err(e) => {
                warn!(path = %log_path.display(), error = %e, "cannot open content log; Steam source idle");
                let _ = shutdown.changed().await;
                return Ok(());
            }
        };
        let mut cursor = reader
            .get_ref()
            .metadata()
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                _ = notify_rx.recv() => {
                    cursor = self.drain_lines(&log_path, &mut reader, cursor, &tx, &mut running).await?;
                }
                () = tokio::time::sleep(POLL_INTERVAL) => {
                    cursor = self.drain_lines(&log_path, &mut reader, cursor, &tx, &mut running).await?;
                }
            }
        }
        Ok(())
    }

    /// Read all new lines since the last call; emit events for state
    /// transitions. Handles log rotation/truncation by re-opening the
    /// file when its length dips below the cursor.
    async fn drain_lines(
        &self,
        path: &Path,
        reader: &mut BufReader<File>,
        mut cursor: u64,
        tx: &mpsc::Sender<GameEvent>,
        running: &mut HashSet<String>,
    ) -> Result<u64> {
        let len = reader.get_ref().metadata().await?.len();
        if len < cursor {
            // Log was rotated or truncated; re-open.
            debug!("content_log.txt shrank; reopening");
            *reader = open_at_start(path).await?;
            cursor = 0;
        }

        let mut line = String::new();
        loop {
            line.clear();
            let bytes = reader.read_line(&mut line).await?;
            if bytes == 0 {
                break;
            }
            cursor += bytes as u64;
            if let Some((appid, is_running)) = parse_state_change(&line) {
                let was_running = running.contains(appid);
                if is_running && !was_running {
                    let display_name = self.resolve_name(appid).await;
                    info!(appid, %display_name, "Steam: game started");
                    running.insert(appid.to_owned());
                    if tx
                        .send(GameEvent::Started {
                            key: GameKey::steam(appid),
                            display_name,
                            at: OffsetDateTime::now_utc(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                } else if !is_running && was_running {
                    info!(appid, "Steam: game stopped");
                    running.remove(appid);
                    if tx
                        .send(GameEvent::Stopped {
                            key: GameKey::steam(appid),
                            at: OffsetDateTime::now_utc(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        Ok(cursor)
    }
}

async fn open_at_end(path: &Path) -> Result<BufReader<File>> {
    let mut file = File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    file.seek(SeekFrom::End(0)).await?;
    Ok(BufReader::new(file))
}

async fn open_at_start(path: &Path) -> Result<BufReader<File>> {
    let file = File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    Ok(BufReader::new(file))
}

/// Parse a Steam content-log state-change line.
///
/// Shape:
/// `[YYYY-MM-DD HH:MM:SS] AppID <digits> state changed : <flag>,<flag>,...`
///
/// Returns `(appid, app_running)` where `app_running` is `true` when the
/// flag list contains the literal string `App Running`. Unrelated log
/// lines return `None`.
fn parse_state_change(line: &str) -> Option<(&str, bool)> {
    let rest = line.split_once("] AppID ")?.1;
    let (appid, rest) = rest.split_once(' ')?;
    if !appid.chars().all(|c| c.is_ascii_digit()) || appid.is_empty() {
        return None;
    }
    let flags = rest.strip_prefix("state changed : ")?;
    let app_running = flags
        .trim_end_matches('\n')
        .split(',')
        .map(str::trim)
        .any(|f| f == "App Running");
    Some((appid, app_running))
}

/// Extract the first value associated with a simple `"key" "value"` line
/// from a VDF document. Does not understand nesting; suitable for the
/// flat records Steam emits for `name` and `StateFlags` in
/// appmanifest files.
fn parse_vdf_top_level_string(content: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    for line in content.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(&needle) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(after_open) = rest.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = after_open.find('"') {
            return Some(after_open[..end].to_owned());
        }
    }
    None
}

fn parse_vdf_top_level_u64(content: &str, key: &str) -> Option<u64> {
    parse_vdf_top_level_string(content, key).and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_state_change_running() {
        let line =
            "[2026-04-18 22:14:43] AppID 1621690 state changed : Fully Installed,App Running,\n";
        assert_eq!(parse_state_change(line), Some(("1621690", true)));
    }

    #[test]
    fn parses_state_change_not_running() {
        let line = "[2026-04-18 22:14:43] AppID 1621690 state changed : Fully Installed,\n";
        assert_eq!(parse_state_change(line), Some(("1621690", false)));
    }

    #[test]
    fn parses_state_change_with_many_flags() {
        let line = "[...] AppID 440 state changed : Fully Installed,Update Queued,App Running,\n";
        assert_eq!(parse_state_change(line), Some(("440", true)));
    }

    #[test]
    fn ignores_non_state_change_lines() {
        assert_eq!(
            parse_state_change("[2026-04-17 21:33:27] Current download rate: 0.000 Mbps\n"),
            None
        );
        assert_eq!(
            parse_state_change(
                "[2026-04-17 21:33:27] AppID 1621690 Shader update changed : None\n"
            ),
            None
        );
    }

    #[test]
    fn ignores_non_numeric_appid() {
        assert_eq!(
            parse_state_change("[x] AppID abc state changed : App Running,\n"),
            None
        );
    }

    #[test]
    fn parses_manifest_name() {
        let content = "\
\"AppState\"
{
\t\"appid\"\t\t\"228980\"
\t\"name\"\t\t\"Steamworks Common Redistributables\"
\t\"StateFlags\"\t\t\"4\"
}";
        assert_eq!(
            parse_vdf_top_level_string(content, "name").as_deref(),
            Some("Steamworks Common Redistributables")
        );
        assert_eq!(parse_vdf_top_level_u64(content, "StateFlags"), Some(4));
    }

    #[test]
    fn parses_manifest_with_running_flag() {
        let content = "\
\"AppState\"
{
\t\"appid\"\t\t\"440\"
\t\"name\"\t\t\"Team Fortress 2\"
\t\"StateFlags\"\t\t\"68\"
}";
        let flags = parse_vdf_top_level_u64(content, "StateFlags").unwrap();
        assert!(flags & APP_RUNNING_FLAG != 0);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn state_change_parser_never_panics(s in "\\PC{0,300}") {
            let _ = parse_state_change(&s);
        }

        #[test]
        fn vdf_string_parser_never_panics(s in "\\PC{0,500}", key in "[a-zA-Z_]{1,20}") {
            let _ = parse_vdf_top_level_string(&s, &key);
        }

        #[test]
        fn vdf_u64_parser_never_panics(s in "\\PC{0,500}", key in "[a-zA-Z_]{1,20}") {
            let _ = parse_vdf_top_level_u64(&s, &key);
        }
    }
}
