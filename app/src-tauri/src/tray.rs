//! System tray integration.
//!
//! Builds a StatusNotifierItem tray icon with a Show / Hide / Quit
//! menu, wires left-click to toggle the main window, and intercepts
//! close-window so the app minimises to the tray instead of exiting.
//! Tooltip text flips between the idle state and "session active"
//! based on the `ludex:session-started` / `ludex:session-ended`
//! events that [`crate::bridge`] forwards from the daemon's D-Bus
//! signals.
//!
//! We use [`ksni`] rather than Tauri's built-in `tray-icon` because
//! the latter pulls in the abandoned `libappindicator-rs` crate
//! (last commit 2022) which wraps the deprecated
//! `libayatana-appindicator` C library — producing a deprecation
//! warning on every startup. `ksni` is a pure-Rust implementation
//! of the StatusNotifierItem spec and talks directly to the D-Bus
//! host (KDE Plasma, GNOME-with-extension, Cinnamon, Xfce, Budgie),
//! so no C dependency is involved.
//!
//! The tooltip deliberately does *not* include the game's name —
//! that would require a D-Bus `GetApplication` RPC for every
//! `session-started` event, and Tauri's `listen_any` callback is
//! synchronous. Adding name resolution is a follow-up behind a
//! worker channel.

use std::sync::{Arc, OnceLock};

use ksni::menu::StandardItem;
use ksni::{Handle, Icon, MenuItem, ToolTip, Tray, TrayMethods};
use tauri::{AppHandle, Listener, Manager, Runtime, WindowEvent};

use crate::bridge::{EVENT_SESSION_ENDED, EVENT_SESSION_STARTED};

const MAIN_WINDOW: &str = "main";
const TOOLTIP_IDLE: &str = "ludex";
const TOOLTIP_ACTIVE: &str = "ludex · session active";

struct LudexTray<R: Runtime> {
    app: AppHandle<R>,
    icon: Icon,
    tooltip_title: String,
}

impl<R: Runtime> Tray for LudexTray<R> {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn title(&self) -> String {
        "ludex".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.tooltip_title.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        toggle_main(&self.app);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Show".into(),
                activate: Box::new(|this: &mut Self| show_main(&this.app)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Hide".into(),
                activate: Box::new(|this: &mut Self| hide_main(&this.app)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| this.app.exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Spawn the StatusNotifierItem service, install the close-to-tray
/// hook on the main window, and register listeners that flip the
/// tooltip on session events.
pub(crate) fn install<R: Runtime>(app: &AppHandle<R>) -> anyhow::Result<()> {
    // Convert Tauri's default window icon to ksni's ARGB32 byte
    // layout — Tauri stores RGBA8, the StatusNotifierItem spec
    // wants ARGB32.
    let tauri_icon = app
        .default_window_icon()
        .ok_or_else(|| anyhow::anyhow!("default window icon missing"))?;
    let icon = to_ksni_icon(tauri_icon.rgba(), tauri_icon.width(), tauri_icon.height())?;

    let tray = LudexTray {
        app: app.clone(),
        icon,
        tooltip_title: TOOLTIP_IDLE.into(),
    };

    // Spawning is async; the handle is filled in once the service
    // is up. Listener callbacks read the OnceLock; if it's empty
    // (service hasn't finished starting yet, or failed), they no-op.
    let handle_slot: Arc<OnceLock<Handle<LudexTray<R>>>> = Arc::new(OnceLock::new());
    let handle_slot_for_spawn = Arc::clone(&handle_slot);
    tauri::async_runtime::spawn(async move {
        match tray.spawn().await {
            Ok(handle) => {
                let _ = handle_slot_for_spawn.set(handle);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "StatusNotifierItem tray failed to start; continuing without tray"
                );
            }
        }
    });

    // Close on the main window hides rather than exits, leaving the
    // tray as the remaining surface. "Show" from the menu, or a
    // tray click, restores it.
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let window_for_close = window.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window_for_close.hide();
            }
        });
    }

    // Flip tooltip on session events. Callbacks are synchronous; we
    // spawn the async update onto Tauri's runtime.
    let slot_started = Arc::clone(&handle_slot);
    app.listen_any(EVENT_SESSION_STARTED, move |_event| {
        update_tooltip(&slot_started, TOOLTIP_ACTIVE);
    });
    let slot_ended = Arc::clone(&handle_slot);
    app.listen_any(EVENT_SESSION_ENDED, move |_event| {
        update_tooltip(&slot_ended, TOOLTIP_IDLE);
    });

    Ok(())
}

fn update_tooltip<R: Runtime>(slot: &Arc<OnceLock<Handle<LudexTray<R>>>>, text: &'static str) {
    let Some(handle) = slot.get().cloned() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        handle
            .update(|t: &mut LudexTray<R>| {
                text.clone_into(&mut t.tooltip_title);
            })
            .await;
    });
}

fn to_ksni_icon(rgba: &[u8], width: u32, height: u32) -> anyhow::Result<Icon> {
    let width = i32::try_from(width)?;
    let height = i32::try_from(height)?;
    if !rgba.len().is_multiple_of(4) {
        anyhow::bail!("icon pixel buffer is not a multiple of 4 bytes");
    }
    let mut data = rgba.to_vec();
    // RGBA8 → ARGB32 network byte order: rotate each pixel right by
    // one byte so [R, G, B, A] becomes [A, R, G, B].
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    Ok(Icon {
        width,
        height,
        data,
    })
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
    // `is_visible` can in principle error; treat that as "show the
    // window" so the user isn't stuck with a hidden app that won't
    // respond to its own tray icon.
    if let Ok(true) = window.is_visible() {
        let _ = window.hide();
    } else {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
