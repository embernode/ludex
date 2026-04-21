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
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("ludex").join("ludex.sqlite"))
}
