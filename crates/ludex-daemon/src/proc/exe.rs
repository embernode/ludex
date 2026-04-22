//! Resolve `/proc/<pid>/exe` to the canonical on-disk path of the
//! process's main executable.

use std::io;
use std::path::PathBuf;

/// Return the target of `/proc/<pid>/exe`. Fails with `NotFound` if the
/// process no longer exists and `PermissionDenied` when we cannot read
/// the symlink (some hardened distros restrict this for processes we
/// don't own).
pub async fn read(pid: u32) -> io::Result<PathBuf> {
    tokio::fs::read_link(format!("/proc/{pid}/exe")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn self_exe_resolves() {
        let me = read(std::process::id()).await.expect("own exe readable");
        assert!(me.is_absolute());
    }

    #[tokio::test]
    async fn missing_pid_returns_error() {
        // PID 0 is reserved by the kernel and never has /proc entries.
        assert!(read(0).await.is_err());
    }
}
