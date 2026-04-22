//! Walk a process's ancestry via `/proc/<pid>/stat` and
//! `/proc/<pid>/comm`.
//!
//! Used by the Gamescope ancestry check and, more generally, by any
//! detector that needs to know "is this process a descendant of X".

use std::io;

/// Return the parent PID of `pid` by reading `/proc/<pid>/stat`, or
/// `None` when the process cannot be read (dead, restricted, or PID 0).
pub fn parent(pid: u32) -> io::Result<Option<u32>> {
    if pid == 0 {
        return Ok(None);
    }
    let content = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    Ok(parse_ppid(&content))
}

/// Parse the `ppid` field out of a `/proc/<pid>/stat` line.
///
/// The tricky bit is the `comm` field, which is parenthesised but can
/// contain embedded parens and spaces. We take the substring after the
/// *last* `)`, which is always the field separator between comm and
/// state.
pub(crate) fn parse_ppid(stat: &str) -> Option<u32> {
    let rparen = stat.rfind(')')?;
    let rest = stat[rparen + 1..].trim_start();
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let ppid_str = fields.next()?;
    ppid_str.parse().ok()
}

/// Return the executable name of a process via `/proc/<pid>/comm`,
/// trimmed. Fails with the underlying I/O error when the file cannot
/// be read.
pub fn comm(pid: u32) -> io::Result<String> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/comm"))?;
    Ok(s.trim().to_owned())
}

/// Iterator yielding every ancestor PID of `pid` in order, starting
/// with the immediate parent and walking toward PID 1 (`init`). Stops
/// on any I/O error or when the parent chain terminates.
pub fn ancestors(pid: u32) -> impl Iterator<Item = u32> {
    std::iter::successors(parent(pid).ok().flatten(), |&p| {
        if p <= 1 {
            None
        } else {
            parent(p).ok().flatten()
        }
    })
}

/// `true` if any ancestor of `pid` has the given `/proc/<pid>/comm`
/// value.
pub fn has_ancestor_comm(pid: u32, target_comm: &str) -> bool {
    ancestors(pid).any(|p| comm(p).is_ok_and(|c| c == target_comm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_stat_line() {
        // pid comm state ppid pgrp session ...
        let line = "42 (bash) S 1234 42 42 34816 42 4194304 ...";
        assert_eq!(parse_ppid(line), Some(1234));
    }

    #[test]
    fn handles_parens_in_comm() {
        // Synthetic — a process whose executable name contains ') ('.
        let line = "42 (weird) (name) S 1234 42 ...";
        assert_eq!(parse_ppid(line), Some(1234));
    }

    #[test]
    fn malformed_stat_returns_none() {
        assert_eq!(parse_ppid("not a stat line"), None);
        assert_eq!(parse_ppid(""), None);
        assert_eq!(parse_ppid("42 (bash"), None);
    }

    #[test]
    fn self_has_a_parent() {
        let ppid = parent(std::process::id()).unwrap();
        assert!(ppid.is_some());
        assert_ne!(ppid, Some(0));
    }

    #[test]
    fn init_has_no_parent_in_the_walk() {
        // PID 1 is init; ancestors() walks away from self and stops at
        // or before 1. Either way, 0 must not appear.
        let chain: Vec<u32> = ancestors(std::process::id()).collect();
        assert!(!chain.contains(&0));
    }

    #[test]
    fn nonexistent_pid_parent_errors() {
        assert!(parent(u32::MAX).is_err());
    }
}
