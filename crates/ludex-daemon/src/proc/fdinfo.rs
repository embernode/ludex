//! Parse `/proc/<pid>/fdinfo/*` for per-process DRM GPU statistics.
//!
//! The kernel DRM subsystem writes per-fd accounting into
//! `/proc/<pid>/fdinfo/<fd>`. For fds belonging to a DRM device we see
//! lines like:
//!
//! ```text
//! drm-driver:   amdgpu
//! drm-pdev:     0000:03:00.0
//! drm-client-id: 12345
//! drm-engine-gfx: 12345678 ns
//! drm-memory-vram: 1024 kB
//! drm-memory-gtt:  2048 kB
//! ```
//!
//! A process may own multiple DRM fds (presentation + rendering on
//! different queues), and the values per-fd are cumulative. The summary
//! reported here sums memory across fds and total engine time — the two
//! signals the decision gate needs.
//!
//! Supported on modern kernels across AMD/Intel/NVIDIA (NVIDIA 550+).

use std::io;

/// Summary of the DRM fdinfo counters aggregated across every file
/// descriptor the process has open against a DRM device.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GpuSummary {
    /// First DRM driver name observed (`"amdgpu"`, `"i915"`,
    /// `"nvidia-drm"`, …). `None` means no DRM fd was found for this
    /// process.
    pub driver: Option<String>,
    /// Sum of all `drm-memory-*` values in bytes across every fd.
    pub memory_bytes: u64,
    /// Sum of all `drm-engine-*` values in nanoseconds. Does not include
    /// per-engine capacity fields.
    pub engine_nanoseconds: u64,
}

impl GpuSummary {
    /// `true` if any DRM fd was observed.
    #[must_use]
    pub const fn any(&self) -> bool {
        self.driver.is_some()
    }
}

/// Read every `/proc/<pid>/fdinfo/<fd>` entry and aggregate the DRM
/// counters found.
pub async fn read(pid: u32) -> io::Result<GpuSummary> {
    let dir = format!("/proc/{pid}/fdinfo");
    let mut rd = tokio::fs::read_dir(&dir).await?;
    let mut summary = GpuSummary::default();
    while let Some(entry) = rd.next_entry().await? {
        // Individual fdinfo files race (fd closes between readdir and
        // open); treat them as absent rather than propagating the
        // error.
        let Ok(content) = tokio::fs::read_to_string(entry.path()).await else {
            continue;
        };
        accumulate(&content, &mut summary);
    }
    Ok(summary)
}

/// Parse a single fdinfo document and add its DRM counters to `summary`.
pub fn accumulate(content: &str, summary: &mut GpuSummary) {
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        if key == "drm-driver" {
            if summary.driver.is_none() && !value.is_empty() {
                summary.driver = Some(value.to_owned());
            }
        } else if key.starts_with("drm-memory-") {
            if let Some(bytes) = parse_byte_unit(value) {
                summary.memory_bytes = summary.memory_bytes.saturating_add(bytes);
            }
        } else if key.starts_with("drm-engine-") && !key.ends_with("-capacity") {
            if let Some(ns) = parse_ns(value) {
                summary.engine_nanoseconds = summary.engine_nanoseconds.saturating_add(ns);
            }
        }
    }
}

fn parse_byte_unit(value: &str) -> Option<u64> {
    let (num, unit) = value.split_once(' ')?;
    let n: u64 = num.parse().ok()?;
    let multiplier = match unit.trim() {
        "B" | "bytes" => 1,
        "kB" | "KB" | "KiB" => 1024,
        "MB" | "MiB" => 1024 * 1024,
        "GB" | "GiB" => 1024 * 1024 * 1024,
        _ => return None,
    };
    Some(n.saturating_mul(multiplier))
}

fn parse_ns(value: &str) -> Option<u64> {
    let (num, unit) = value.split_once(' ')?;
    if unit.trim() != "ns" {
        return None;
    }
    num.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(content: &str) -> GpuSummary {
        let mut s = GpuSummary::default();
        accumulate(content, &mut s);
        s
    }

    #[test]
    fn parses_amdgpu_fdinfo() {
        let content = "\
pos: 0
flags: 02
mnt_id: 24
drm-driver: amdgpu
drm-pdev: 0000:03:00.0
drm-client-id: 12345
drm-engine-gfx: 12345678 ns
drm-memory-vram: 1024 kB
drm-memory-gtt: 2048 kB
";
        let s = parse_one(content);
        assert_eq!(s.driver.as_deref(), Some("amdgpu"));
        assert_eq!(s.memory_bytes, (1024 + 2048) * 1024);
        assert_eq!(s.engine_nanoseconds, 12_345_678);
    }

    #[test]
    fn sums_across_multiple_fds() {
        let fd1 = "drm-driver: amdgpu\ndrm-memory-vram: 100 kB\ndrm-engine-gfx: 500 ns\n";
        let fd2 = "drm-driver: amdgpu\ndrm-memory-vram: 300 kB\ndrm-engine-gfx: 1500 ns\n";
        let mut s = GpuSummary::default();
        accumulate(fd1, &mut s);
        accumulate(fd2, &mut s);
        assert_eq!(s.memory_bytes, 400 * 1024);
        assert_eq!(s.engine_nanoseconds, 2000);
    }

    #[test]
    fn non_drm_fdinfo_yields_nothing() {
        let content = "pos: 0\nflags: 02\nmnt_id: 24\n";
        let s = parse_one(content);
        assert_eq!(s, GpuSummary::default());
        assert!(!s.any());
    }

    #[test]
    fn excludes_engine_capacity_lines() {
        // `drm-engine-*-capacity` is metadata, not work — must not be
        // added into engine_nanoseconds.
        let content = "\
drm-driver: amdgpu
drm-engine-gfx: 1000 ns
drm-engine-gfx-capacity: 999999 ns
";
        let s = parse_one(content);
        assert_eq!(s.engine_nanoseconds, 1000);
    }

    #[test]
    fn accepts_common_memory_units() {
        for (value, expected) in [
            ("1 B", 1),
            ("1 kB", 1024),
            ("1 KiB", 1024),
            ("2 MB", 2 * 1024 * 1024),
            ("3 MiB", 3 * 1024 * 1024),
        ] {
            assert_eq!(parse_byte_unit(value), Some(expected));
        }
    }

    #[test]
    fn rejects_unknown_unit() {
        assert_eq!(parse_byte_unit("1 pages"), None);
    }

    #[test]
    fn garbage_input_is_harmless() {
        let mut s = GpuSummary::default();
        accumulate("not: valid: input\n::\n\n", &mut s);
        assert_eq!(s, GpuSummary::default());
    }
}
