//! SQLite-level backup operations for the ludex database.
//!
//! Uses SQLite's `VACUUM INTO` to produce consistent single-file
//! snapshots. VACUUM INTO runs against live WAL state, which means
//! the daemon can keep running — its writes during the backup just
//! end up in the next snapshot, not the one in flight. A plain
//! `cp` is unsafe here because the WAL journal lives in a sibling
//! file that may be mid-commit.
//!
//! These functions are deliberately small and total so the daemon's
//! periodic backup task, the CLI's manual `backup now`, and a future
//! restore path can all share the same primitives.

use std::fs;
use std::path::{Path, PathBuf};

use time::format_description::FormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

use crate::db::Database;
use crate::error::{Error, Result};
use crate::paths::default_backup_dir;
use crate::repo::{BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_RETENTION_COUNT};

/// Prefix every backup filename carries. Used by [`list_backups`] to
/// filter unrelated files in the backup directory (dotfiles, stray
/// user copies) without matching on the `.sqlite` extension alone.
pub const BACKUP_FILENAME_PREFIX: &str = "ludex-";
/// Extension every backup filename carries.
pub const BACKUP_FILENAME_EXTENSION: &str = "sqlite";

// Compact ISO 8601 basic profile — date and time, no separators.
// Every filename is stamped in UTC by the writer and parsed back
// as UTC by the reader, so we don't thread the offset through the
// format string; the literal `Z` is appended/stripped manually.
// Example: `20260423T140532Z`.
const FILENAME_TIMESTAMP_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]");

/// One snapshot file, as surfaced by [`list_backups`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    /// Full path on disk.
    pub path: PathBuf,
    /// Parsed timestamp from the filename. `None` when the filename
    /// carries the ludex prefix but a timestamp segment we can't
    /// parse — such rows still count toward pruning but are
    /// reported with their mtime instead at the caller's option.
    pub timestamp: Option<OffsetDateTime>,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Format a backup filename for `at`. Converts to UTC first so
/// every filename is parseable by [`parse_backup_filename`] without
/// accepting arbitrary offsets into the filesystem namespace.
#[must_use]
pub fn format_backup_filename(at: OffsetDateTime) -> String {
    let stamp = at
        .to_offset(time::UtcOffset::UTC)
        .format(FILENAME_TIMESTAMP_FORMAT)
        .unwrap_or_else(|_| "unknown".to_owned());
    format!("{BACKUP_FILENAME_PREFIX}{stamp}Z.{BACKUP_FILENAME_EXTENSION}")
}

/// Recover the timestamp encoded in a backup filename, or `None` if
/// the name doesn't match the expected shape.
#[must_use]
pub fn parse_backup_filename(name: &str) -> Option<OffsetDateTime> {
    let rest = name.strip_prefix(BACKUP_FILENAME_PREFIX)?;
    let rest = rest.strip_suffix(&format!(".{BACKUP_FILENAME_EXTENSION}"))?;
    // Literal `Z` is attached by `format_backup_filename`; strip it
    // here and assume UTC. Parsing via `PrimitiveDateTime` avoids
    // the format-string dance for embedding an offset marker.
    let rest = rest.strip_suffix('Z')?;
    PrimitiveDateTime::parse(rest, FILENAME_TIMESTAMP_FORMAT)
        .ok()
        .map(PrimitiveDateTime::assume_utc)
}

/// Write a consistent snapshot of `db` to `dst`. Creates the
/// destination's parent directory as needed.
///
/// Fails with [`Error::Invariant`] when the destination already
/// exists — snapshots are immutable, and silently overwriting one
/// would mask a scheduling collision.
pub async fn create_snapshot(db: &Database, dst: &Path) -> Result<()> {
    if dst.exists() {
        return Err(Error::Invariant("backup destination already exists"));
    }
    if let Some(parent) = dst.parent() {
        // std::fs is fine here — creating one directory is a few
        // syscalls and we're not in a hot path. Avoids pulling
        // tokio into ludex-core for one call site.
        fs::create_dir_all(parent)?;
    }
    // SQLite's VACUUM INTO takes any expression that evaluates to a
    // string, so a bound parameter works — no manual quoting, no
    // risk of path-injection weirdness even for future callers
    // that might pass user-supplied destinations.
    let path = dst.to_string_lossy().into_owned();
    sqlx::query("VACUUM INTO ?")
        .bind(path)
        .execute(db.pool())
        .await?;
    Ok(())
}

/// Enumerate every backup file in `dir`, newest first. Files that
/// don't start with [`BACKUP_FILENAME_PREFIX`] or whose extension
/// is not [`BACKUP_FILENAME_EXTENSION`] are skipped — we don't
/// want to accidentally list or prune a user's unrelated SQLite
/// copies that happen to live in the same directory.
pub fn list_backups(dir: &Path) -> Result<Vec<BackupEntry>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for dirent in fs::read_dir(dir)? {
        let dirent = dirent?;
        let name = dirent.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let has_prefix = name.starts_with(BACKUP_FILENAME_PREFIX);
        let has_extension = Path::new(name)
            .extension()
            .is_some_and(|e| e == BACKUP_FILENAME_EXTENSION);
        if !(has_prefix && has_extension) {
            continue;
        }
        let metadata = dirent.metadata()?;
        entries.push(BackupEntry {
            path: dirent.path(),
            timestamp: parse_backup_filename(name),
            size_bytes: metadata.len(),
        });
    }
    // Filename is lex-sortable → reverse for newest-first. Entries
    // with unparseable timestamps fall to the end deterministically.
    entries.sort_by(|a, b| b.path.file_name().cmp(&a.path.file_name()));
    Ok(entries)
}

/// Delete every backup in `dir` beyond the most recent `keep`.
/// Returns the paths that were removed, in deletion order.
/// `keep == 0` would wipe the whole set; we clamp that case at 1
/// to avoid a surprising `prune` that leaves nothing recoverable.
pub fn prune_backups(dir: &Path, keep: usize) -> Result<Vec<PathBuf>> {
    let keep = keep.max(1);
    let entries = list_backups(dir)?;
    if entries.len() <= keep {
        return Ok(Vec::new());
    }
    let mut removed = Vec::new();
    for entry in entries.into_iter().skip(keep) {
        if let Err(e) = fs::remove_file(&entry.path) {
            // Swallow a missing-file race (another prune from the
            // CLI) but surface anything else.
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(Error::Io(e));
            }
        }
        removed.push(entry.path);
    }
    Ok(removed)
}

/// Take a snapshot at `$XDG_DATA_HOME/ludex/backups/` using the
/// standard filename format, then prune to the retention count
/// stored in `SettingsRepo` (or `retention_override` when the
/// caller wants to bypass the stored value).
///
/// Returns the path of the new snapshot. Shared between the daemon
/// scheduler and the CLI's `ludex backup now`.
pub async fn snapshot_now(db: &Database, retention_override: Option<usize>) -> Result<PathBuf> {
    let dir =
        default_backup_dir().ok_or(Error::Invariant("neither XDG_DATA_HOME nor HOME is set"))?;
    let retention = if let Some(n) = retention_override {
        n
    } else {
        let n = db
            .settings()
            .get_u64(BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_RETENTION_COUNT)
            .await?;
        usize::try_from(n).unwrap_or(usize::MAX)
    };
    let dst = dir.join(format_backup_filename(OffsetDateTime::now_utc()));
    create_snapshot(db, &dst).await?;
    let _ = prune_backups(&dir, retention)?;
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn filename_roundtrips() {
        let t = datetime!(2026-04-23 14:05:32 UTC);
        let name = format_backup_filename(t);
        assert!(name.starts_with("ludex-"));
        assert!(name.ends_with(".sqlite"));
        assert_eq!(parse_backup_filename(&name), Some(t));
    }

    #[test]
    fn parse_rejects_unrelated_filenames() {
        assert!(parse_backup_filename("other.sqlite").is_none());
        assert!(parse_backup_filename("ludex-garbage.sqlite").is_none());
        assert!(parse_backup_filename("ludex-20260423T140532Z.db").is_none());
    }
}
