//! Watch a process for termination via `pidfd_open` + `AsyncFd`.
//!
//! A `pidfd` is a file descriptor that becomes readable when the
//! referenced process exits. This is a kernel-supervised signal, so
//! it fires regardless of what the compositor or anything else in
//! userspace is doing.
//!
//! Used by the KWin source to catch the case where a game process
//! exits without the foreground changing (e.g. a short-lived game
//! that terminates before another window takes focus), so the
//! session closes promptly rather than waiting for the next
//! activation.

use std::io;
use std::os::fd::AsRawFd;

use rustix::process::{pidfd_open, Pid, PidfdFlags};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Spawn a task that waits for `pid` to exit, then sends `pid` on
/// `exit_tx`. Returns `Ok(())` on successful spawn. Failures to open
/// the pidfd are logged and silently swallowed — the foreground
/// source can still detect the exit via a window focus change, just
/// with a small delay.
pub fn watch(pid: u32, exit_tx: mpsc::UnboundedSender<u32>) -> io::Result<()> {
    let raw = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid does not fit in i32"))?;
    let Some(parsed) = Pid::from_raw(raw) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "zero pid"));
    };

    let fd = match pidfd_open(parsed, PidfdFlags::NONBLOCK) {
        Ok(f) => f,
        Err(e) => {
            // Common outcomes: ESRCH (already exited), EACCES (no
            // permission). Both are normal for short-lived or
            // cross-user processes.
            warn!(pid, error = %e, "pidfd_open failed; process-exit monitoring off for this pid");
            return Err(e.into());
        }
    };
    debug!(pid, fd = fd.as_raw_fd(), "pidfd opened for exit watch");

    let async_fd = AsyncFd::new(fd)?;
    tokio::spawn(async move {
        match async_fd.readable().await {
            Ok(_guard) => {
                debug!(pid, "pidfd became readable; process exited");
            }
            Err(e) => {
                warn!(pid, error = %e, "pidfd readable() failed; forwarding exit anyway");
            }
        }
        let _ = exit_tx.send(pid);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn detects_child_exit() {
        // Spawn a short-lived child, watch it, confirm we get a
        // notification with its pid.
        let mut child = tokio::process::Command::new("true")
            .spawn()
            .expect("true(1) must be available");
        let pid = child.id().expect("child has pid");

        let (tx, mut rx) = mpsc::unbounded_channel();
        watch(pid, tx).unwrap();

        // Let the child exit and the tokio task fire.
        let _ = child.wait().await;

        let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("exit notification should arrive within two seconds")
            .expect("channel still open");
        assert_eq!(received, pid);
    }

    #[tokio::test]
    async fn invalid_pid_returns_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        // PID 0 is reserved and cannot back a pidfd.
        assert!(watch(0, tx).is_err());
    }
}
