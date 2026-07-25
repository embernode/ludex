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
mod tray;

/// Start the Tauri application. Blocks the current thread until the
/// window closes.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let bridge = Arc::new(TrackerBridge::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        // Opens the repo link in the user's default browser via xdg-
        // open; no network egress from ludex itself. The matching JS
        // binding is `@tauri-apps/plugin-opener::openUrl`.
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::clone(&bridge))
        .setup(move |app| {
            // Spawn the signal forwarder on Tauri's async runtime
            // so D-Bus signals from the daemon surface as Tauri
            // events the frontend can listen for.
            let handle = app.handle().clone();
            let forwarder_bridge = Arc::clone(&bridge);
            tauri::async_runtime::spawn(async move {
                bridge::run_signal_forwarder(handle, forwarder_bridge).await;
            });

            // Install the tray icon. Must run after the main window
            // is registered so `get_webview_window("main")` resolves.
            // The bridge handle lets the tray's tooltip worker call
            // GetApplication(id) to resolve the active game's name.
            tray::install(app.handle(), &bridge)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bridge::list_applications,
            bridge::get_application,
            bridge::list_recent_sessions,
            bridge::list_sessions_in_range,
            bridge::list_sessions_for_application,
            bridge::list_daily_playtime,
            bridge::list_blocked_application_ids,
            bridge::block_application,
            bridge::unblock_application,
            bridge::delete_session,
            bridge::get_gpu_memory_threshold_bytes,
            bridge::set_gpu_memory_threshold_bytes,
            bridge::get_alt_tab_grace_seconds,
            bridge::set_alt_tab_grace_seconds,
            bridge::get_pause_when_backgrounded,
            bridge::set_pause_when_backgrounded,
            bridge::get_idle_grace_seconds,
            bridge::set_idle_grace_seconds,
            bridge::get_backup_interval_hours,
            bridge::set_backup_interval_hours,
            bridge::get_backup_retention_count,
            bridge::set_backup_retention_count,
            bridge::take_backup_now,
            bridge::get_backup_stats,
            bridge::open_backup_directory,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
