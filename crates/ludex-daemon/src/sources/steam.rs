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
use ludex_core::vdf::{
    parse_top_level_string as parse_vdf_top_level_string,
    parse_top_level_u64 as parse_vdf_top_level_u64,
};
use ludex_core::GameKey;
use notify::{recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use time::OffsetDateTime;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, instrument, warn};

use crate::event::GameEvent;

/// Steam `App Running` bit in the appmanifest `StateFlags` bitmask.
const APP_RUNNING_FLAG: u64 = 64;

/// Fallback poll cadence in case the filesystem watcher misses an event.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum bytes to read from the content log in a single drain pass.
/// Guards against a pathological gap between cursor and file length;
/// whatever's left is picked up on the next pass.
const MAX_DRAIN_BYTES: u64 = 4 * 1024 * 1024;

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
                // Game title logged at debug only — journalctl and
                // stderr capture go to info+ by default, and the
                // title isn't needed for operational correlation.
                debug!(appid, %display_name, "cold-start: detected running game");
                info!(appid, "cold-start: detected running game");
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

        // Start reading from wherever the log currently ends — historical
        // lines are not interesting at cold-start.
        let mut cursor = match tokio::fs::metadata(&log_path).await {
            Ok(m) => m.len(),
            Err(e) => {
                warn!(path = %log_path.display(), error = %e, "cannot stat content log; Steam source idle");
                let _ = shutdown.changed().await;
                return Ok(());
            }
        };

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                _ = notify_rx.recv() => {
                    cursor = self.drain_lines(&log_path, cursor, &tx, &mut running).await?;
                }
                () = tokio::time::sleep(POLL_INTERVAL) => {
                    cursor = self.drain_lines(&log_path, cursor, &tx, &mut running).await?;
                }
            }
        }
        Ok(())
    }

    /// Read every complete line appended since `cursor`, emit events for
    /// the state transitions they describe, and return the new cursor.
    ///
    /// The cursor only advances past bytes ending in `\n`. A partial
    /// write (Steam flushed `"...App Run"` but not yet `"ning,\n"`) is
    /// left in place so the next wake picks up the full line once the
    /// trailing bytes land — advancing past unterminated bytes used to
    /// silently drop events when file flushes didn't land on a line
    /// boundary.
    ///
    /// Log rotation / truncation is detected by the file shrinking
    /// below the cursor; the cursor resets to zero and we rescan from
    /// the top.
    async fn drain_lines(
        &self,
        path: &Path,
        cursor: u64,
        tx: &mpsc::Sender<GameEvent>,
        running: &mut HashSet<String>,
    ) -> Result<u64> {
        let mut file = File::open(path)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        let len = file.metadata().await?.len();

        let mut cursor = if len < cursor {
            debug!("content_log.txt shrank; rescanning from start");
            0
        } else {
            cursor
        };
        if len == cursor {
            return Ok(cursor);
        }

        file.seek(SeekFrom::Start(cursor)).await?;
        let available = len - cursor;
        let to_read = available.min(MAX_DRAIN_BYTES);
        let mut buf = Vec::with_capacity(to_read as usize);
        file.take(to_read).read_to_end(&mut buf).await?;

        // Keep only the bytes up to (and including) the final newline;
        // whatever follows is a partial line that the next drain pass
        // will pick up.
        let consumed = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        if consumed == 0 {
            return Ok(cursor);
        }

        let Ok(terminated) = std::str::from_utf8(&buf[..consumed]) else {
            // Content log is ASCII in practice; a non-UTF-8 chunk means
            // a mid-line write with bytes we can't split safely. Skip
            // this pass and let the next wake re-read when the flush
            // completes.
            debug!("content_log.txt chunk is not valid UTF-8; deferring");
            return Ok(cursor);
        };

        for line in terminated.split_inclusive('\n') {
            if let Some((appid, is_running)) = parse_state_change(line) {
                let was_running = running.contains(appid);
                if is_running && !was_running {
                    let display_name = self.resolve_name(appid).await;
                    debug!(appid, %display_name, "Steam: game started");
                    info!(appid, "Steam: game started");
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
        cursor += consumed as u64;
        Ok(cursor)
    }
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

    /// A partial line at the end of the file (the bytes between the
    /// last `\n` and EOF) must not be consumed — `drain_lines` leaves
    /// the cursor positioned before it so the next pass picks up the
    /// full line once the trailing bytes land. Advancing past those
    /// bytes (the old behaviour) silently dropped events that
    /// straddled a flush boundary.
    #[tokio::test]
    async fn drain_lines_leaves_unterminated_tail_for_next_pass() {
        use tokio::io::AsyncWriteExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("content_log.txt");
        let source = SteamSource::new(tmp.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel::<GameEvent>(8);
        let mut running = HashSet::new();

        // First write: one complete line, then a partial one with no
        // trailing newline. Only the complete line should be consumed.
        let mut f = tokio::fs::File::create(&log).await.unwrap();
        f.write_all(b"[t] AppID 440 state changed : App Running,\n")
            .await
            .unwrap();
        f.write_all(b"[t] AppID 730 state chang").await.unwrap();
        f.flush().await.unwrap();

        let mut cursor = source
            .drain_lines(&log, 0, &tx, &mut running)
            .await
            .unwrap();

        // Exactly the bytes up to and including the first \n.
        assert_eq!(cursor, 43);
        assert!(running.contains("440"));
        assert!(!running.contains("730"));
        let first = rx.try_recv().expect("Started for 440");
        assert!(matches!(first, GameEvent::Started { .. }));
        assert!(rx.try_recv().is_err());

        // Second write: completes the deferred line and adds a new one.
        // Next drain should process both.
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .await
            .unwrap();
        f.write_all(b"ed : Fully Installed,App Running,\n")
            .await
            .unwrap();
        f.write_all(b"[t] AppID 440 state changed : Fully Installed,\n")
            .await
            .unwrap();
        f.flush().await.unwrap();

        cursor = source
            .drain_lines(&log, cursor, &tx, &mut running)
            .await
            .unwrap();

        assert!(
            running.contains("730"),
            "730 started after partial line completed"
        );
        assert!(
            !running.contains("440"),
            "440 stopped when App Running bit cleared"
        );
        // Both transitions should have produced events.
        let _ = rx.try_recv().expect("Started for 730");
        let _ = rx.try_recv().expect("Stopped for 440");
        // Cursor now at EOF.
        let len = tokio::fs::metadata(&log).await.unwrap().len();
        assert_eq!(cursor, len);
    }

    /// Rotation: if the file shrinks below the cursor, start over from
    /// the top of the new content.
    #[tokio::test]
    async fn drain_lines_handles_rotation() {
        use tokio::io::AsyncWriteExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("content_log.txt");
        let source = SteamSource::new(tmp.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel::<GameEvent>(8);
        let mut running = HashSet::new();

        tokio::fs::write(&log, b"[t] AppID 440 state changed : App Running,\n")
            .await
            .unwrap();
        let cursor = source
            .drain_lines(&log, 0, &tx, &mut running)
            .await
            .unwrap();
        assert!(running.contains("440"));
        let _ = rx.try_recv().unwrap();

        // Simulate rotation: rewrite the file with shorter content
        // under an entirely new appid. Cursor > new_len must trigger a
        // rescan from the start.
        let mut f = tokio::fs::File::create(&log).await.unwrap();
        f.write_all(b"[t] AppID 9 state changed : App Running,\n")
            .await
            .unwrap();
        f.flush().await.unwrap();

        let new_cursor = source
            .drain_lines(&log, cursor, &tx, &mut running)
            .await
            .unwrap();
        assert!(running.contains("9"));
        assert!(new_cursor > 0);
        let _ = rx.try_recv().expect("Started for 9 after rotation");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn state_change_parser_never_panics(s in "\\PC{0,300}") {
            let _ = parse_state_change(&s);
        }
    }
}
