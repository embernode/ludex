//! Filesystem-location helpers shared by the daemon and the CLI.

use std::path::PathBuf;

/// Canonical per-user database path.
///
/// Resolves `$XDG_DATA_HOME/ludex/ludex.sqlite`, falling back to
/// `$HOME/.local/share/ludex/ludex.sqlite` if `XDG_DATA_HOME` is unset.
/// Returns `None` when neither `XDG_DATA_HOME` nor `HOME` is set in the
/// environment, which should not happen on a normal user session.
#[must_use]
pub fn default_database_path() -> Option<PathBuf> {
    ludex_data_dir().map(|d| d.join("ludex.sqlite"))
}

/// Canonical per-user backup directory.
///
/// Resolves `$XDG_DATA_HOME/ludex/backups/` (or the `$HOME/.local/share`
/// fallback). Same-root-as-the-database choice is deliberate: moving
/// the XDG_DATA_HOME takes the backups along with the live DB, and
/// a `ludex` subdir uninstall cleanly removes everything.
#[must_use]
pub fn default_backup_dir() -> Option<PathBuf> {
    ludex_data_dir().map(|d| d.join("backups"))
}

fn ludex_data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("ludex"))
}
