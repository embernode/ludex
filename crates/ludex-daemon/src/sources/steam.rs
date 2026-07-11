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
    parse_all_values as parse_vdf_all_values, parse_top_level_string as parse_vdf_top_level_string,
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
use crate::proc::environ;

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

    /// Every `steamapps` directory Steam manages: the primary one plus
    /// any secondary libraries (games installed on other drives) listed
    /// in `steamapps/libraryfolders.vdf`.
    ///
    /// Falls back to just the primary directory when the index is
    /// missing or unparseable — the single-library behaviour that
    /// predated multi-library support. The primary is always first and
    /// duplicates are elided, so a library whose `path` is the Steam
    /// dir itself isn't scanned twice.
    async fn library_steamapps_dirs(&self) -> Vec<PathBuf> {
        let primary = self.steamapps_dir();
        let mut dirs = vec![primary.clone()];
        let index = primary.join("libraryfolders.vdf");
        if let Ok(content) = tokio::fs::read_to_string(&index).await {
            for path in parse_vdf_all_values(&content, "path") {
                let steamapps = PathBuf::from(path).join("steamapps");
                if !dirs.contains(&steamapps) {
                    dirs.push(steamapps);
                }
            }
        }
        dirs
    }

    /// Best-effort name lookup from `appmanifest_<appid>.acf`, searched
    /// across every Steam library (the manifest lives in whichever
    /// library the game is installed on, not necessarily the primary).
    async fn resolve_name(&self, appid: &str) -> String {
        for dir in self.library_steamapps_dirs().await {
            let manifest = dir.join(format!("appmanifest_{appid}.acf"));
            if let Ok(s) = tokio::fs::read_to_string(&manifest).await {
                if let Some(name) = parse_vdf_top_level_string(&s, "name") {
                    return name;
                }
            }
        }
        format!("AppID {appid}")
    }

    /// Scan all installed appmanifests across every Steam library and
    /// emit [`GameEvent::Started`] for each whose `StateFlags` has the
    /// App Running bit set. Silently skips unreadable files.
    async fn cold_start_scan(
        &self,
        tx: &mpsc::Sender<GameEvent>,
        running: &mut HashSet<String>,
    ) -> Result<()> {
        // Resolve which appids actually have a live process, so a
        // stale `App Running` bit left in a manifest by a hard crash
        // doesn't spawn a phantom session (GATE-5). `None` means we
        // couldn't enumerate `/proc` at all — in that case we don't
        // trust the guard and fall back to emitting on the bit alone,
        // since suppressing a *real* running game is the worse failure.
        let live_appids = Self::live_steam_appids().await;
        self.cold_start_scan_with(tx, running, live_appids.as_ref())
            .await
    }

    async fn cold_start_scan_with(
        &self,
        tx: &mpsc::Sender<GameEvent>,
        running: &mut HashSet<String>,
        live_appids: Option<&HashSet<String>>,
    ) -> Result<()> {
        for dir in self.library_steamapps_dirs().await {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(d) => d,
                Err(e) => {
                    warn!(path = %dir.display(), error = %e, "steamapps dir unreadable; skipping library");
                    continue;
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
                    // GATE-5: the `App Running` bit can survive a hard
                    // crash that never rewrote the manifest, leaving a
                    // "running" game with no process. Cross-check a live
                    // process before trusting it. `live_appids` is
                    // `None` only when `/proc` couldn't be enumerated at
                    // all — then we don't second-guess the bit, because
                    // dropping a real running game is worse than a rare
                    // phantom.
                    if let Some(live) = live_appids {
                        if !live.contains(appid) {
                            info!(
                                appid,
                                "cold-start: App-Running bit set but no live process; skipping stale manifest"
                            );
                            continue;
                        }
                    }
                    // Emit each appid at most once per scan. A game can
                    // surface in more than one scanned dir if the primary
                    // library is also listed in libraryfolders.vdf under a
                    // differently-spelled path (trailing slash, symlink),
                    // which the path-string dedup wouldn't catch.
                    if !running.insert(appid.to_owned()) {
                        continue;
                    }
                    let display_name = parse_vdf_top_level_string(&content, "name")
                        .unwrap_or_else(|| format!("AppID {appid}"));
                    // Game title logged at debug only — journalctl and
                    // stderr capture go to info+ by default, and the
                    // title isn't needed for operational correlation.
                    debug!(appid, %display_name, "cold-start: detected running game");
                    info!(appid, "cold-start: detected running game");
                    if tx
                        .send(GameEvent::Started {
                            key: GameKey::steam(appid),
                            display_name,
                            executable_path: None,
                            at: OffsetDateTime::now_utc(),
                        })
                        .await
                        .is_err()
                    {
                        // Receiver dropped; abort the whole scan.
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// Scan `/proc` once for the set of Steam appids that currently
    /// have a live process. Steam stamps `SteamAppId` (and Proton adds
    /// `STEAM_COMPAT_APP_ID`) into every game process's environment and
    /// its descendants, so a genuinely running game — native or Proton,
    /// at any depth in the process tree — surfaces here regardless of
    /// which pid holds the window. Used to reject a stale `App Running`
    /// manifest bit left behind by a hard crash (GATE-5).
    ///
    /// Returns `None` only when `/proc` itself can't be enumerated, so
    /// the caller can distinguish "scanned, nothing live" (suppress the
    /// phantom) from "couldn't check" (don't second-guess the bit). A
    /// single process whose `environ` is unreadable (gone mid-scan, or
    /// a foreign uid) just doesn't contribute; game processes are the
    /// daemon user's own, so their environ is readable in practice.
    async fn live_steam_appids() -> Option<HashSet<String>> {
        let mut entries = match tokio::fs::read_dir("/proc").await {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "cannot enumerate /proc for Steam liveness; emitting on the App-Running bit alone");
                return None;
            }
        };
        let mut appids = HashSet::new();
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    // A mid-scan enumeration error means we can't claim
                    // to have seen every process, so we must not report
                    // a partial set as authoritative — that would let a
                    // not-yet-read running game be suppressed. Fall back
                    // to trusting the App-Running bit (return `None`).
                    warn!(error = %e, "error enumerating /proc mid-scan; not trusting the liveness guard");
                    return None;
                }
            };
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(env) = environ::read(pid).await else {
                continue;
            };
            for key in ["SteamAppId", "STEAM_COMPAT_APP_ID"] {
                if let Some(value) = env.get(key) {
                    // Real appids are nonzero digits; "0" is the
                    // non-Steam-shortcut sentinel (no appmanifest anyway).
                    if value != "0" && !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
                    {
                        appids.insert(value.clone());
                    }
                }
            }
        }
        Some(appids)
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
        // Keep one sender alive for the life of this task: when the
        // watcher is absent (no log yet) or its installation failed,
        // `notify_rx.recv()` must pend forever rather than resolve
        // `None` in a hot loop.
        let _notify_keepalive = notify_tx.clone();
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
            info!(path = %log_path.display(), "content_log.txt not present yet; polling for it");
            None
        };

        self.cold_start_scan(&tx, &mut running).await?;

        // Start reading from wherever the log currently ends — historical
        // lines are not interesting at cold-start. A missing file (Steam
        // installed but not launched this boot) starts the cursor at
        // zero: once Steam creates the log, everything in it is new.
        let mut cursor = match tokio::fs::metadata(&log_path).await {
            Ok(m) => m.len(),
            Err(_) => 0,
        };

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                _ = notify_rx.recv() => {
                    cursor = self.drain_or_keep(&log_path, cursor, &tx, &mut running).await;
                }
                () = tokio::time::sleep(POLL_INTERVAL) => {
                    cursor = self.drain_or_keep(&log_path, cursor, &tx, &mut running).await;
                }
            }
        }
        Ok(())
    }

    /// [`Self::drain_lines`] with errors downgraded to a log line and
    /// the cursor left in place. A transient I/O failure — the log not
    /// created yet, a rotation's rename/recreate window — must not kill
    /// the source for the rest of the daemon's life; the next watcher
    /// event or poll tick simply retries.
    async fn drain_or_keep(
        &self,
        path: &Path,
        cursor: u64,
        tx: &mpsc::Sender<GameEvent>,
        running: &mut HashSet<String>,
    ) -> u64 {
        match self.drain_lines(path, cursor, tx, running).await {
            Ok(next) => next,
            Err(e) => {
                // A missing file is the steady state until Steam's first
                // launch of the boot; only unexpected failures warrant a
                // warning.
                let not_found = e
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                if not_found {
                    debug!(path = %path.display(), "content log not present; will retry");
                } else {
                    warn!(error = %e, "draining content log failed; keeping cursor and retrying");
                }
                cursor
            }
        }
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
                            executable_path: None,
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

    /// A game installed on a secondary Steam library (another drive) and
    /// already running at daemon start must be picked up by the
    /// cold-start scan. Its manifest lives only under the secondary
    /// library's `steamapps`, discovered via `libraryfolders.vdf`
    /// (GATE-4). Before the multi-library fix the scan only read the
    /// primary dir and the game was invisible.
    #[tokio::test]
    async fn cold_start_scan_finds_games_in_secondary_library() {
        let primary = tempfile::tempdir().unwrap();
        let secondary = tempfile::tempdir().unwrap();

        // Primary library exists but holds no running game.
        let primary_steamapps = primary.path().join("steamapps");
        tokio::fs::create_dir_all(&primary_steamapps).await.unwrap();

        // libraryfolders.vdf lists the Steam dir itself plus the
        // secondary drive.
        let libraryfolders = format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}",
            primary.path().display(),
            secondary.path().display(),
        );
        tokio::fs::write(primary_steamapps.join("libraryfolders.vdf"), libraryfolders)
            .await
            .unwrap();

        // The running game's manifest lives only in the secondary library.
        let secondary_steamapps = secondary.path().join("steamapps");
        tokio::fs::create_dir_all(&secondary_steamapps)
            .await
            .unwrap();
        let manifest = "\
\"AppState\"
{
\t\"appid\"\t\t\"999\"
\t\"name\"\t\t\"Test Game\"
\t\"StateFlags\"\t\t\"68\"
}";
        tokio::fs::write(secondary_steamapps.join("appmanifest_999.acf"), manifest)
            .await
            .unwrap();

        let source = SteamSource::new(primary.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel::<GameEvent>(8);
        let mut running = HashSet::new();
        // Treat appid 999 as live so the liveness guard passes it
        // through — this test is about library enumeration, not GATE-5.
        let live = HashSet::from(["999".to_string()]);
        source
            .cold_start_scan_with(&tx, &mut running, Some(&live))
            .await
            .unwrap();
        drop(tx);

        let ev = rx
            .recv()
            .await
            .expect("a Started event for the secondary-library game");
        match ev {
            GameEvent::Started {
                key, display_name, ..
            } => {
                assert_eq!(key, GameKey::steam("999"));
                assert_eq!(display_name, "Test Game");
            }
            other @ GameEvent::Stopped { .. } => panic!("expected Started, got {other:?}"),
        }
        assert!(running.contains("999"));
    }

    /// GATE-5: a manifest with the `App Running` bit set but no live
    /// process (stale bit after a hard crash) must NOT emit a Started —
    /// the liveness set was scanned and the appid isn't in it.
    #[tokio::test]
    async fn cold_start_scan_skips_running_bit_without_live_process() {
        let steam = tempfile::tempdir().unwrap();
        let steamapps = steam.path().join("steamapps");
        tokio::fs::create_dir_all(&steamapps).await.unwrap();
        let manifest = "\
\"AppState\"
{
\t\"appid\"\t\t\"777\"
\t\"name\"\t\t\"Crashed Game\"
\t\"StateFlags\"\t\t\"68\"
}";
        tokio::fs::write(steamapps.join("appmanifest_777.acf"), manifest)
            .await
            .unwrap();

        let source = SteamSource::new(steam.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel::<GameEvent>(8);
        let mut running = HashSet::new();
        // Scanned successfully, but nothing live → the stale bit is
        // suppressed.
        let live = HashSet::new();
        source
            .cold_start_scan_with(&tx, &mut running, Some(&live))
            .await
            .unwrap();
        drop(tx);

        assert!(
            rx.recv().await.is_none(),
            "a running-bit manifest with no live process must not emit Started",
        );
        assert!(
            !running.contains("777"),
            "the phantom appid must not be marked running",
        );
    }

    /// GATE-5 fallback: when `/proc` couldn't be scanned (`None`), the
    /// guard is bypassed and the `App Running` bit is trusted — dropping
    /// a genuinely running game is worse than a rare phantom.
    #[tokio::test]
    async fn cold_start_scan_trusts_bit_when_liveness_unknown() {
        let steam = tempfile::tempdir().unwrap();
        let steamapps = steam.path().join("steamapps");
        tokio::fs::create_dir_all(&steamapps).await.unwrap();
        let manifest = "\
\"AppState\"
{
\t\"appid\"\t\t\"555\"
\t\"name\"\t\t\"Running Game\"
\t\"StateFlags\"\t\t\"68\"
}";
        tokio::fs::write(steamapps.join("appmanifest_555.acf"), manifest)
            .await
            .unwrap();

        let source = SteamSource::new(steam.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel::<GameEvent>(8);
        let mut running = HashSet::new();
        source
            .cold_start_scan_with(&tx, &mut running, None)
            .await
            .unwrap();
        drop(tx);

        match rx.recv().await {
            Some(GameEvent::Started { key, .. }) => assert_eq!(key, GameKey::steam("555")),
            other => panic!("expected Started for 555, got {other:?}"),
        }
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

    /// A missing `content_log.txt` at daemon start (Steam installed
    /// but not launched this boot) must not park the source forever:
    /// when Steam later creates the file, the poll loop has to pick
    /// it up and events have to flow. Paused time lets the 5-second
    /// poll cadence elapse instantly.
    #[tokio::test(start_paused = true)]
    async fn run_picks_up_content_log_created_after_start() {
        use tokio::io::AsyncWriteExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let source = SteamSource::new(tmp.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel::<GameEvent>(8);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(source.run(tx, shutdown_rx));

        // Let the source finish cold-start and initialise its read
        // cursor while the log is still absent (cursor starts at 0).
        // A bare `yield_now` is too tight: if the file gains its line
        // before the source reads `metadata(log).len()`, the cursor
        // starts past that line and the source correctly treats it as
        // pre-existing history — deterministically skipping it. Sleep
        // long enough for the (fast, file-less) cold-start to complete.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Steam launches: the log appears with a running game.
        tokio::fs::create_dir_all(tmp.path().join("logs"))
            .await
            .unwrap();
        let mut f = tokio::fs::File::create(tmp.path().join("logs/content_log.txt"))
            .await
            .unwrap();
        f.write_all(b"[t] AppID 440 state changed : App Running,\n")
            .await
            .unwrap();
        f.flush().await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("source should pick up the late-created log")
            .expect("event channel open");
        assert!(matches!(
            event,
            GameEvent::Started { ref key, .. } if key == &GameKey::steam("440")
        ));

        shutdown_tx.send(true).unwrap();
        handle.await.unwrap().unwrap();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn state_change_parser_never_panics(s in "\\PC{0,300}") {
            let _ = parse_state_change(&s);
        }
    }
}
