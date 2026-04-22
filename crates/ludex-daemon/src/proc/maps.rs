//! Parse `/proc/<pid>/maps` and detect which graphics libraries are
//! mapped into the process.
//!
//! `/proc/<pid>/maps` lists every memory mapping with a trailing
//! pathname column:
//!
//! ```text
//! 7f8d12345000-7f8d12346000 r-xp 00000000 fd:01 12345 /usr/lib/libGL.so.1
//! ```
//!
//! We scan pathnames for a small set of well-known shared objects and
//! DLL names. The detection is a strong gate on Linux: ordinary desktop
//! apps link Qt or GTK, neither of which pulls in raw GL / Vulkan /
//! SDL at runtime. Observing any of these in `maps` is a solid signal
//! the process is doing hardware-accelerated rendering.

use std::io;
use std::path::Path;

/// Which graphics stacks a given process has mapped in.
///
/// The four stacks are genuinely independent (a Proton game can mix GL
/// and Vulkan via DXVK, for example), so a separate boolean per stack
/// is the natural shape despite what `clippy::struct_excessive_bools`
/// thinks.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one flag per independent graphics stack is the correct API shape"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraphicsLibraries {
    /// `libGL.so.*`, `libEGL.so.*`, or `libGLX.so.*` mapped.
    pub opengl: bool,
    /// `libvulkan.so.*` mapped.
    pub vulkan: bool,
    /// `libSDL2-*.so.*` or `libSDL.so.*` mapped.
    pub sdl: bool,
    /// Proton/Wine translation DLLs mapped (`dxvk`, `vkd3d`, `wined3d`,
    /// or Wine's own `d3d*.dll` / `dxgi.dll`).
    pub directx_via_proton: bool,
}

impl GraphicsLibraries {
    /// Returns `true` if any graphics library was detected.
    #[must_use]
    pub const fn any(self) -> bool {
        self.opengl || self.vulkan || self.sdl || self.directx_via_proton
    }
}

/// Read `/proc/<pid>/maps` and detect mapped graphics libraries.
pub async fn read(pid: u32) -> io::Result<GraphicsLibraries> {
    let content = tokio::fs::read_to_string(format!("/proc/{pid}/maps")).await?;
    Ok(parse(&content))
}

/// Parse a `/proc/<pid>/maps` document into a detected-library summary.
pub fn parse(maps: &str) -> GraphicsLibraries {
    let mut g = GraphicsLibraries::default();
    for line in maps.lines() {
        let Some(path) = pathname_column(line) else {
            continue;
        };
        classify(path, &mut g);
    }
    g
}

/// The pathname column is everything past the fifth whitespace-separated
/// field; returns `None` for anonymous mappings.
fn pathname_column(line: &str) -> Option<&str> {
    // Fields: addr perms offset dev inode path
    let mut fields = line
        .splitn(6, char::is_whitespace)
        .filter(|s| !s.is_empty());
    let _ = fields.next()?; // addr range
    let _ = fields.next()?; // perms
    let _ = fields.next()?; // offset
    let _ = fields.next()?; // dev
    let _ = fields.next()?; // inode
    let tail = fields.next()?.trim();
    if tail.is_empty() {
        None
    } else {
        Some(tail)
    }
}

fn classify(path: &str, g: &mut GraphicsLibraries) {
    let name = Path::new(path)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(path)
        .to_ascii_lowercase();

    if name.starts_with("libgl.so")
        || name.starts_with("libglx.so")
        || name.starts_with("libegl.so")
    {
        g.opengl = true;
    }
    if name.starts_with("libvulkan.so") {
        g.vulkan = true;
    }
    if name.starts_with("libsdl") {
        g.sdl = true;
    }
    // Proton translation. DXVK/VKD3D/wined3d filenames are stable; Wine
    // also ships built-in replacements named d3d9.dll, d3d11.dll, etc.
    if name.contains("dxvk")
        || name.contains("vkd3d")
        || name.contains("wined3d")
        || matches!(
            name.as_str(),
            "d3d8.dll" | "d3d9.dll" | "d3d10.dll" | "d3d11.dll" | "d3d12.dll" | "dxgi.dll"
        )
    {
        g.directx_via_proton = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_opengl_mapping() {
        let s = "7f00-7f01 r-xp 00000000 fd:01 1 /usr/lib/libGL.so.1\n";
        assert!(parse(s).opengl);
    }

    #[test]
    fn classifies_vulkan_mapping() {
        let s = "7f00-7f01 r-xp 00000000 fd:01 1 /usr/lib64/libvulkan.so.1.3\n";
        assert!(parse(s).vulkan);
    }

    #[test]
    fn classifies_sdl_mapping() {
        let s = "7f00-7f01 r-xp 00000000 fd:01 1 /usr/lib/libSDL2-2.0.so.0\n";
        assert!(parse(s).sdl);
    }

    #[test]
    fn classifies_proton_dxvk() {
        let s = "7f00-7f01 r-xp 00000000 fd:01 1 /home/u/.steam/compatdata/440/pfx/drive_c/windows/system32/dxvk_d3d9.dll\n";
        assert!(parse(s).directx_via_proton);
    }

    #[test]
    fn classifies_raw_d3d11_dll() {
        let s = "7f00-7f01 r-xp 00000000 fd:01 1 Z:/some/path/d3d11.dll\n";
        assert!(parse(s).directx_via_proton);
    }

    #[test]
    fn ignores_unrelated_libraries() {
        let s = "\
7f00-7f01 r-xp 00000000 fd:01 1 /usr/lib/libc.so.6
7f02-7f03 r-xp 00000000 fd:01 2 /usr/lib/libQt6Core.so.6
";
        let g = parse(s);
        assert_eq!(g, GraphicsLibraries::default());
    }

    #[test]
    fn ignores_anonymous_mappings() {
        let s = "7f00-7f01 rw-p 00000000 00:00 0\n";
        let g = parse(s);
        assert_eq!(g, GraphicsLibraries::default());
    }

    #[test]
    fn ignores_stacked_regions() {
        let s = "7f00-7f01 rw-p 00000000 00:00 0 [heap]\n7f02-7f03 rw-p 0 00:00 0 [stack]\n";
        let g = parse(s);
        assert_eq!(g, GraphicsLibraries::default());
    }

    #[test]
    fn any_flag() {
        let mut g = GraphicsLibraries::default();
        assert!(!g.any());
        g.vulkan = true;
        assert!(g.any());
    }

    #[test]
    fn garbage_input_never_panics() {
        parse("");
        parse("not valid /proc/maps content");
        parse("\0\0\0\n\n\n");
    }
}
