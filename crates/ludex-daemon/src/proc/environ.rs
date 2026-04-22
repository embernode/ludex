//! Parse `/proc/<pid>/environ` — a process's environment block.
//!
//! The file is a null-separated sequence of `KEY=VALUE` entries.
//! Non-UTF-8 entries are skipped; every variable relevant to
//! launcher attribution is set to ASCII by the tools that set it.

use std::collections::HashMap;
use std::io;

/// Read `/proc/<pid>/environ` and return a `KEY → VALUE` map.
pub async fn read(pid: u32) -> io::Result<HashMap<String, String>> {
    let bytes = tokio::fs::read(format!("/proc/{pid}/environ")).await?;
    Ok(parse(&bytes))
}

/// Parse a null-separated `/proc/<pid>/environ` document.
pub fn parse(content: &[u8]) -> HashMap<String, String> {
    content
        .split(|b| *b == 0)
        .filter_map(|entry| {
            if entry.is_empty() {
                return None;
            }
            let idx = entry.iter().position(|b| *b == b'=')?;
            let key = std::str::from_utf8(&entry[..idx]).ok()?.to_owned();
            let value = std::str::from_utf8(&entry[idx + 1..]).ok()?.to_owned();
            Some((key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_environ() {
        let raw = b"FOO=bar\0BAZ=qux\0\0";
        let env = parse(raw);
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(env.get("BAZ").map(String::as_str), Some("qux"));
    }

    #[test]
    fn ignores_entries_without_equals() {
        let raw = b"GOOD=yes\0MALFORMED\0ALSO_GOOD=true\0";
        let env = parse(raw);
        assert_eq!(env.get("GOOD").map(String::as_str), Some("yes"));
        assert_eq!(env.get("ALSO_GOOD").map(String::as_str), Some("true"));
        assert_eq!(env.get("MALFORMED"), None);
    }

    #[test]
    fn ignores_invalid_utf8() {
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"OK=value\0");
        raw.extend_from_slice(b"BAD=");
        raw.push(0xff);
        raw.push(0);
        let env = parse(&raw);
        assert_eq!(env.get("OK").map(String::as_str), Some("value"));
        assert_eq!(env.get("BAD"), None);
    }

    #[test]
    fn empty_input() {
        assert_eq!(parse(&[]).len(), 0);
        assert_eq!(parse(b"\0\0\0").len(), 0);
    }

    #[test]
    fn value_may_contain_equals() {
        let raw = b"PATH=/a=b/c\0";
        let env = parse(raw);
        assert_eq!(env.get("PATH").map(String::as_str), Some("/a=b/c"));
    }

    #[tokio::test]
    async fn self_environ_reads() {
        let env = read(std::process::id()).await.unwrap();
        assert!(!env.is_empty());
        // PATH is set in every sane test environment.
        assert!(env.contains_key("PATH"));
    }
}
