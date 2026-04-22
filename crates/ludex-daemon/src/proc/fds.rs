//! Enumerate `/proc/<pid>/fd/*` — the open files of a target process.
//!
//! Each entry under `fd/` is a symlink to the resource backing that
//! file descriptor. We resolve each symlink and return the targets that
//! look like real paths. This is the primitive the emulator-ROM
//! detector (M4.x) uses: given a known emulator process, find the ROM
//! file it currently has open by matching against its configured glob
//! patterns.

use std::io;
use std::path::PathBuf;

/// Return the resolved targets of every file descriptor under
/// `/proc/<pid>/fd/`. fds whose targets are not plain filesystem paths
/// (`anon_inode:...`, `socket:[...]`) are included verbatim; callers
/// typically filter with [`Path::is_absolute`].
pub async fn list(pid: u32) -> io::Result<Vec<PathBuf>> {
    let dir = format!("/proc/{pid}/fd");
    let mut rd = tokio::fs::read_dir(&dir).await?;
    let mut targets = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        if let Ok(target) = tokio::fs::read_link(entry.path()).await {
            targets.push(target);
        }
    }
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn sees_tempfile_we_hold_open() {
        let mut tf = tempfile::NamedTempFile::new().unwrap();
        tf.write_all(b"hello").unwrap();
        let path = tf.path().to_path_buf();

        let fds = list(std::process::id()).await.unwrap();
        let found = fds.iter().any(|p| p == &path);
        assert!(
            found,
            "expected {} among /proc/self/fd/* targets, got {fds:?}",
            path.display()
        );
    }

    #[tokio::test]
    async fn missing_pid_errors() {
        assert!(list(0).await.is_err());
    }
}
