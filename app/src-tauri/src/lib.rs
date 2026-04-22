//! ludex desktop UI (Tauri + SvelteKit).
//!
//! The Rust side is deliberately thin: Tauri hosts a webview that
//! serves the SvelteKit app, and exposes a D-Bus proxy for
//! `net.ludex.Tracker1` through a small set of
//! `#[tauri::command]` bridges (added in M6.3). This module is the
//! entry point; [`run`] wires the plugins and hands control to
//! Tauri's event loop.

#![warn(missing_docs)]

/// Start the Tauri application. Blocks the current thread until
/// the window closes.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
