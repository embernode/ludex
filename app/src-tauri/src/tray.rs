//! System tray integration.
//!
//! Builds a status-area icon with a Show / Hide / Quit menu, wires
//! left-click to toggle the main window, and intercepts close-window
//! so the app minimises to the tray instead of exiting. Tooltip text
//! flips between the idle state and "session active" based on the
//! `ludex:session-started` / `ludex:session-ended` events that
//! [`crate::bridge`] forwards from the daemon's D-Bus signals.
//!
//! On Linux this runs through `libayatana-appindicator3`, which is
//! already required by the Tauri system-dep set.
//!
//! The tooltip deliberately does *not* include the game's name — that
//! would require a D-Bus RPC (`GetApplication`) for every
//! `session-started` event, and Tauri's `listen_any` callback is
//! synchronous. Adding name resolution is a follow-up behind a
//! worker channel.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Listener, Manager, Runtime, WindowEvent};

use crate::bridge::{EVENT_SESSION_ENDED, EVENT_SESSION_STARTED};

const TRAY_ID: &str = "main";
const MAIN_WINDOW: &str = "main";
const TOOLTIP_IDLE: &str = "ludex";
const TOOLTIP_ACTIVE: &str = "ludex · session active";

/// Build the tray icon, wire its menu + click behaviour, install the
/// close-to-tray hook on the main window, and register listeners
/// that flip the tooltip on session events.
pub(crate) fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide_item = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &hide_item, &sep, &quit_item])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".to_owned()))?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(TOOLTIP_IDLE)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "hide" => hide_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main(tray.app_handle());
            }
        })
        .build(app)?;

    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let window_for_close = window.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Prevent the window from closing the app. Hiding it
                // leaves the tray icon as the only remaining surface
                // — clicking it or the "Show" menu item restores.
                api.prevent_close();
                let _ = window_for_close.hide();
            }
        });
    }

    let idle_handle = app.clone();
    app.listen_any(EVENT_SESSION_STARTED, move |_event| {
        set_tooltip(&idle_handle, TOOLTIP_ACTIVE);
    });
    let active_handle = app.clone();
    app.listen_any(EVENT_SESSION_ENDED, move |_event| {
        set_tooltip(&active_handle, TOOLTIP_IDLE);
    });

    Ok(())
}

fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn hide_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.hide();
    }
}

fn toggle_main<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        Ok(false) => {
            let _ = window.show();
            let _ = window.set_focus();
        }
        Err(_) => {
            // Unknown state — default to surfacing the window so the
            // user isn't stuck with a hidden app that won't respond.
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

fn set_tooltip<R: Runtime>(app: &AppHandle<R>, text: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(text));
    }
}
