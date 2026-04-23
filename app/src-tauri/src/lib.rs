//! ludex desktop UI (Tauri + SvelteKit).
//!
//! The Rust host is deliberately thin: Tauri opens a webview that
//! serves the SvelteKit bundle, and [`bridge`] exposes the daemon's
//! `net.ludex.Tracker1` D-Bus API through Tauri commands and events.
//! All layout, state, and presentation live in Svelte.

#![warn(missing_docs)]

use std::sync::Arc;

use bridge::TrackerBridge;

mod bridge;

/// Start the Tauri application. Blocks the current thread until the
/// window closes.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let bridge = Arc::new(TrackerBridge::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        .manage(Arc::clone(&bridge))
        .setup(move |app| {
            // Spawn the signal forwarder on Tauri's async runtime
            // so D-Bus signals from the daemon surface as Tauri
            // events the frontend can listen for.
            let handle = app.handle().clone();
            let bridge = Arc::clone(&bridge);
            tauri::async_runtime::spawn(async move {
                bridge::run_signal_forwarder(handle, bridge).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bridge::list_applications,
            bridge::get_application,
            bridge::list_recent_sessions,
            bridge::list_sessions_for_application,
            bridge::list_daily_playtime,
            bridge::list_blocked_application_ids,
            bridge::block_application,
            bridge::unblock_application,
            bridge::get_gpu_memory_threshold_bytes,
            bridge::set_gpu_memory_threshold_bytes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
