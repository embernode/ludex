#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ensure_webkit_wayland_compat();
    ludex_gui_lib::run();
}

/// Install environment shims WebKitGTK needs to run cleanly on the
/// desktops ludex targets.
///
/// WebKitGTK's DMABUF renderer (the default since ~2.44) crashes with
/// `Error 71 (Protocol error)` when talking to NVIDIA's proprietary
/// Wayland driver. Other renderers work. Forcing the non-DMABUF path
/// is the community-standard workaround — it costs a small amount of
/// compositing performance and no feature loss for a mostly-text
/// application like ludex. Harmless on Intel/AMD/Mesa where the
/// DMABUF path is healthy anyway, and harmless on X11.
///
/// We only set each variable when not already set, so a user who
/// has opinions about WebKit rendering can override.
///
/// Upstream: https://bugs.webkit.org/show_bug.cgi?id=271261
fn ensure_webkit_wayland_compat() {
    const DEFAULTS: &[(&str, &str)] = &[("WEBKIT_DISABLE_DMABUF_RENDERER", "1")];
    for (key, value) in DEFAULTS {
        if std::env::var_os(key).is_none() {
            // SAFETY: `set_var` is unsafe because it races with
            // concurrent `getenv` from other threads. We are the
            // first statement in `main`; no threads have been
            // spawned yet, so no such race exists.
            #[allow(unsafe_code, reason = "single-threaded context at program startup")]
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
}
