//! `ludex backup` subcommands.
//!
//! Writes go through `ludex_core::backup::snapshot_now`, which is
//! the same primitive the daemon's scheduler uses — one code path
//! for periodic and manual snapshots keeps retention behaviour
//! consistent.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ludex_core::backup::{
    list_backups, prune_backups, restore as restore_snapshot, snapshot_now, BackupEntry,
};
use ludex_core::repo::{BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_RETENTION_COUNT};
use ludex_core::{default_backup_dir, default_database_path, Database};
use time::format_description::FormatItem;
use time::macros::format_description;

/// Pretty-print format for `backup list`.
const LIST_TIMESTAMP_FORMAT: &[FormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second] UTC");

/// Take one snapshot now and prune to the configured retention.
pub(crate) async fn now() -> Result<()> {
    let db = open_live_db().await?;
    let Some(db) = db else { return Ok(()) };
    let dst = snapshot_now(&db, None).await.context("snapshot")?;
    db.close().await;
    println!("{}", dst.display());
    Ok(())
}

/// Print the available snapshots, newest-first, in a compact table.
#[allow(
    clippy::unused_async,
    reason = "signature kept async to match the rest of the command dispatch"
)]
pub(crate) async fn list() -> Result<()> {
    let Some(dir) = default_backup_dir() else {
        eprintln!("neither XDG_DATA_HOME nor HOME is set");
        return Ok(());
    };
    let entries = list_backups(&dir).context("list backup directory")?;
    if entries.is_empty() {
        println!(
            "(no backups in {} yet; run `ludex backup now` or let the daemon snapshot)",
            dir.display()
        );
        return Ok(());
    }
    print_table(&entries);
    Ok(())
}

/// Prune to the configured retention (or the `--keep` override).
pub(crate) async fn prune(keep: Option<u64>) -> Result<()> {
    let Some(dir) = default_backup_dir() else {
        eprintln!("neither XDG_DATA_HOME nor HOME is set");
        return Ok(());
    };
    let retention = if let Some(n) = keep {
        usize::try_from(n).unwrap_or(usize::MAX)
    } else if let Some(db) = open_live_db().await? {
        let n = db
            .settings()
            .get_u64(BACKUP_RETENTION_COUNT, DEFAULT_BACKUP_RETENTION_COUNT)
            .await
            .unwrap_or(DEFAULT_BACKUP_RETENTION_COUNT);
        db.close().await;
        usize::try_from(n).unwrap_or(usize::MAX)
    } else {
        // No live DB — use the compiled-in default so a user with
        // a full backup dir and no active install can still prune.
        usize::try_from(DEFAULT_BACKUP_RETENTION_COUNT).unwrap_or(usize::MAX)
    };
    let removed = prune_backups(&dir, retention).context("prune")?;
    if removed.is_empty() {
        println!("nothing to prune; retention {retention}");
    } else {
        for path in &removed {
            println!("removed {}", path.display());
        }
        println!("pruned {} file(s); retained {retention}", removed.len());
    }
    Ok(())
}

/// Restore a snapshot over the live database. Refuses to run while
/// `ludex-daemon` is owning its D-Bus name.
pub(crate) async fn restore(source: PathBuf) -> Result<()> {
    if daemon_active().await {
        anyhow::bail!(
            "ludex-daemon is active — stop it before restoring \
             (e.g. `systemctl --user stop ludex-daemon`)"
        );
    }
    let dst = default_database_path().context("neither XDG_DATA_HOME nor HOME is set")?;
    let source_abs = source
        .canonicalize()
        .with_context(|| format!("resolve source path {}", source.display()))?;
    if let Ok(dst_abs) = dst.canonicalize() {
        if source_abs == dst_abs {
            anyhow::bail!("source and destination are the same file");
        }
    }
    restore_snapshot(&source_abs, &dst)
        .await
        .context("restore")?;
    println!("restored {} to {}", source_abs.display(), dst.display());
    Ok(())
}

async fn open_live_db() -> Result<Option<Database>> {
    let Some(db_path) = default_database_path() else {
        eprintln!("neither XDG_DATA_HOME nor HOME is set");
        return Ok(None);
    };
    if !db_path.exists() {
        eprintln!(
            "no database at {} — has ludex-daemon run yet?",
            db_path.display()
        );
        return Ok(None);
    }
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("open database at {}", db_path.display()))?;
    Ok(Some(db))
}

/// `true` when something owns the `net.ludex.Tracker1` well-known
/// name — the daemon's public D-Bus surface. A false negative (bus
/// unreachable) is safer than a false positive: restore proceeds
/// and the rename step would still fail if anything really held
/// the file.
async fn daemon_active() -> bool {
    let Ok(conn) = zbus::Connection::session().await else {
        return false;
    };
    let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else {
        return false;
    };
    let Ok(names) = proxy.list_names().await else {
        return false;
    };
    names.iter().any(|n| n.as_str() == "net.ludex.Tracker1")
}

fn print_table(entries: &[BackupEntry]) {
    // Matches the 23 chars `LIST_TIMESTAMP_FORMAT` produces:
    // `YYYY-MM-DD HH:MM:SS UTC`.
    const TIMESTAMP_WIDTH: usize = 23;
    const SIZE_WIDTH: usize = 10;
    let path_width = entries
        .iter()
        .map(|e| e.path.display().to_string().chars().count())
        .max()
        .unwrap_or(0)
        .max(4);
    println!(
        "{:<TIMESTAMP_WIDTH$}  {:>SIZE_WIDTH$}  {:<path_width$}",
        "timestamp", "size", "path",
    );
    println!(
        "{}",
        "─".repeat(TIMESTAMP_WIDTH + 2 + SIZE_WIDTH + 2 + path_width)
    );
    for entry in entries {
        let stamp = entry
            .timestamp
            .and_then(|t| t.format(LIST_TIMESTAMP_FORMAT).ok())
            .unwrap_or_else(|| "—".to_owned());
        let size = format_size(entry.size_bytes);
        println!(
            "{stamp:<TIMESTAMP_WIDTH$}  {size:>SIZE_WIDTH$}  {path:<path_width$}",
            path = display_path(&entry.path),
        );
    }
}

fn display_path(p: &Path) -> String {
    p.display().to_string()
}

fn format_size(bytes: u64) -> String {
    // Integer math with one-decimal rendering — no f64 cast needed.
    // Multiplies by 10 before dividing to capture the first decimal
    // place, then splits back out.
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format_one_decimal(bytes, KIB, "KiB")
    } else if bytes < GIB {
        format_one_decimal(bytes, MIB, "MiB")
    } else {
        format_one_decimal(bytes, GIB, "GiB")
    }
}

fn format_one_decimal(value: u64, unit: u64, label: &str) -> String {
    let tenths = value.saturating_mul(10) / unit;
    let whole = tenths / 10;
    let frac = tenths % 10;
    format!("{whole}.{frac} {label}")
}
