//! System tray integration.
//!
//! Builds a StatusNotifierItem tray icon with a Show / Hide / Quit
//! menu, wires left-click to toggle the main window, and intercepts
//! close-window so the app minimises to the tray instead of exiting.
//! Tooltip text reflects the currently-active session — idle shows
//! `ludex`, a running session shows `ludex · <game name>`. The name
//! comes from a `GetApplication(id)` D-Bus call driven by the
//! `ludex:session-started` / `ludex:session-ended` Tauri events
//! that [`crate::bridge`] forwards.
//!
//! Icon follows the system colour scheme: a light icon (dark
//! shapes, for light panels) and a dark icon (light shapes, for
//! dark panels) are both embedded at compile time, and the tray
//! swaps between them based on Tauri's `window.theme()` and its
//! `ThemeChanged` event.
//!
//! We use [`ksni`] rather than Tauri's built-in `tray-icon` because
//! the latter pulls in the abandoned `libappindicator-rs` crate
//! which wraps the deprecated `libayatana-appindicator` C library.
//! `ksni` is a pure-Rust implementation of the StatusNotifierItem
//! spec and talks directly to the D-Bus host, so no C dependency
//! is involved.
//!
//! The Tauri listener callback is synchronous while the name-
//! resolution RPC is async, so the listener only pushes a
//! [`TrayStateUpdate`] onto an unbounded channel — the worker
//! task owned by this module picks it up, calls the bridge's
//! proxy when needed, and updates the tray via the ksni [`Handle`].

use std::sync::{Arc, OnceLock};

use ksni::menu::StandardItem;
use ksni::{Handle, Icon, MenuItem, ToolTip, Tray, TrayMethods};
use tauri::image::Image;
use tauri::{AppHandle, Listener, Manager, Runtime, Theme, WindowEvent};
use tokio::sync::mpsc;

use crate::bridge::{TrackerBridge, EVENT_SESSION_ENDED, EVENT_SESSION_STARTED};

const MAIN_WINDOW: &str = "main";
const TOOLTIP_IDLE: &str = "ludex";
/// Shown between `session-started` and the moment `GetApplication`
/// resolves the name — usually a single D-Bus round trip, but we
/// flash something rather than the idle text so the user sees the
/// session is being tracked.
const TOOLTIP_ACTIVE_UNRESOLVED: &str = "ludex · session active";

/// PNG bytes for the light-theme (dark-shape) and dark-theme
/// (light-shape) icon variants. Kept at 256px so downsampling to
/// typical tray sizes (16/22/24) stays crisp.
const ICON_PNG_LIGHT: &[u8] = include_bytes!("../icons/icon_light.png");
const ICON_PNG_DARK: &[u8] = include_bytes!("../icons/icon.png");

struct ThemeIcons {
    /// Dark shapes on a transparent background — reads on a light
    /// system panel.
    light: Icon,
    /// Light shapes on a transparent background — reads on a dark
    /// system panel.
    dark: Icon,
}

struct LudexTray<R: Runtime> {
    app: AppHandle<R>,
    icons: ThemeIcons,
    is_dark: bool,
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
        vec![if self.is_dark {
            self.icons.dark.clone()
        } else {
            self.icons.light.clone()
        }]
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

/// Message pushed by the Tauri listeners onto the tray worker
/// channel. Listeners are synchronous; the worker drives the async
/// work (name resolution, ksni handle updates) in order.
#[derive(Debug)]
enum TrayStateUpdate {
    TooltipStarted(i64),
    TooltipEnded,
    ThemeChanged(bool),
}

/// Spawn the StatusNotifierItem service, install the close-to-tray
/// hook on the main window, spawn the tray worker, and register
/// listeners that push session + theme events onto its channel.
pub(crate) fn install<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &Arc<TrackerBridge>,
) -> anyhow::Result<()> {
    let icons = ThemeIcons {
        light: decode_icon(ICON_PNG_LIGHT)?,
        dark: decode_icon(ICON_PNG_DARK)?,
    };

    // Initial theme: ask the main window. Tauri's Linux backend
    // returns the webview's reported color scheme, which follows
    // the GTK / freedesktop portal setting. If the platform can't
    // report it (returns Err or None), assume light.
    let initial_is_dark = app
        .get_webview_window(MAIN_WINDOW)
        .and_then(|w| w.theme().ok())
        .is_some_and(|t| matches!(t, Theme::Dark));

    let tray = LudexTray {
        app: app.clone(),
        icons,
        is_dark: initial_is_dark,
        tooltip_title: TOOLTIP_IDLE.into(),
    };

    // Spawning is async; the handle is filled in once the service
    // is up. The worker and the listeners both share the same
    // OnceLock — they see the handle as soon as the spawn finishes
    // and no-op before then.
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

    // The worker owns the bridge handle and serialises updates, so
    // rapid events stay in order even though the D-Bus round-trips
    // are async.
    let (tx, rx) = mpsc::unbounded_channel::<TrayStateUpdate>();
    let handle_slot_for_worker = Arc::clone(&handle_slot);
    let bridge_for_worker = Arc::clone(bridge);
    tauri::async_runtime::spawn(async move {
        run_tray_worker::<R>(rx, handle_slot_for_worker, bridge_for_worker).await;
    });

    // Close on the main window hides rather than exits, leaving
    // the tray as the remaining surface. "Show" from the menu, or
    // a tray click, restores it.
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        // Set the window icon now, and subscribe to theme changes
        // so the title-bar/taskbar icon stays in sync with the
        // tray. On Linux, Tauri emits ThemeChanged when the GTK /
        // portal colour scheme shifts.
        apply_window_icon(&window, initial_is_dark);

        let window_for_close = window.clone();
        let window_for_theme = window.clone();
        let tx_for_window = tx.clone();
        window.on_window_event(move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window_for_close.hide();
            }
            WindowEvent::ThemeChanged(theme) => {
                let is_dark = matches!(theme, Theme::Dark);
                apply_window_icon(&window_for_theme, is_dark);
                let _ = tx_for_window.send(TrayStateUpdate::ThemeChanged(is_dark));
            }
            _ => {}
        });
    }

    // Payload shape: session-started carries a JSON-encoded i64
    // (just the number); session-ended carries a JSON object but
    // we only need the transition, not the id.
    let tx_started = tx.clone();
    app.listen_any(EVENT_SESSION_STARTED, move |event| {
        let Ok(id) = event.payload().parse::<i64>() else {
            return;
        };
        let _ = tx_started.send(TrayStateUpdate::TooltipStarted(id));
    });
    let tx_ended = tx;
    app.listen_any(EVENT_SESSION_ENDED, move |_event| {
        let _ = tx_ended.send(TrayStateUpdate::TooltipEnded);
    });

    Ok(())
}

/// Drain [`TrayStateUpdate`]s from the listener channel. On every
/// `TooltipStarted(id)` we optimistically set the unresolved
/// tooltip so the user has feedback immediately, then look up the
/// name and land the final text. On `TooltipEnded` we reset to
/// idle. On `ThemeChanged` we flip the icon variant.
async fn run_tray_worker<R: Runtime>(
    mut rx: mpsc::UnboundedReceiver<TrayStateUpdate>,
    handle_slot: Arc<OnceLock<Handle<LudexTray<R>>>>,
    bridge: Arc<TrackerBridge>,
) {
    while let Some(update) = rx.recv().await {
        match update {
            TrayStateUpdate::TooltipStarted(id) => {
                apply_tooltip(&handle_slot, TOOLTIP_ACTIVE_UNRESOLVED.to_owned()).await;
                let name = resolve_app_name(&bridge, id).await;
                let text = match name {
                    Some(n) if !n.trim().is_empty() => format!("ludex · {n}"),
                    _ => TOOLTIP_ACTIVE_UNRESOLVED.to_owned(),
                };
                apply_tooltip(&handle_slot, text).await;
            }
            TrayStateUpdate::TooltipEnded => {
                apply_tooltip(&handle_slot, TOOLTIP_IDLE.to_owned()).await;
            }
            TrayStateUpdate::ThemeChanged(is_dark) => {
                apply_theme(&handle_slot, is_dark).await;
            }
        }
    }
}

async fn resolve_app_name(bridge: &Arc<TrackerBridge>, id: i64) -> Option<String> {
    let proxy = match bridge.proxy().await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(error = %e, "tray worker: bridge proxy unavailable");
            return None;
        }
    };
    match proxy.get_application(id).await {
        Ok(apps) => apps.into_iter().next().map(|a| a.product_name),
        Err(e) => {
            tracing::debug!(error = %e, id, "tray worker: GetApplication failed");
            None
        }
    }
}

async fn apply_tooltip<R: Runtime>(
    handle_slot: &Arc<OnceLock<Handle<LudexTray<R>>>>,
    text: String,
) {
    let Some(handle) = handle_slot.get().cloned() else {
        return;
    };
    handle
        .update(move |t: &mut LudexTray<R>| {
            t.tooltip_title = text;
        })
        .await;
}

async fn apply_theme<R: Runtime>(
    handle_slot: &Arc<OnceLock<Handle<LudexTray<R>>>>,
    is_dark: bool,
) {
    let Some(handle) = handle_slot.get().cloned() else {
        return;
    };
    handle
        .update(move |t: &mut LudexTray<R>| {
            t.is_dark = is_dark;
        })
        .await;
}

fn apply_window_icon<R: Runtime>(window: &tauri::WebviewWindow<R>, is_dark: bool) {
    let bytes = if is_dark { ICON_PNG_DARK } else { ICON_PNG_LIGHT };
    match Image::from_bytes(bytes) {
        Ok(img) => {
            if let Err(e) = window.set_icon(img) {
                tracing::debug!(error = %e, "failed to set window icon");
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, "failed to decode embedded window icon");
        }
    }
}

fn decode_icon(png_bytes: &[u8]) -> anyhow::Result<Icon> {
    let img = Image::from_bytes(png_bytes)?;
    to_ksni_icon(img.rgba(), img.width(), img.height())
}

fn to_ksni_icon(rgba: &[u8], width: u32, height: u32) -> anyhow::Result<Icon> {
    let width = i32::try_from(width)?;
    let height = i32::try_from(height)?;
    if !rgba.len().is_multiple_of(4) {
        anyhow::bail!("icon pixel buffer is not a multiple of 4 bytes");
    }
    let mut data = rgba.to_vec();
    // RGBA8 → ARGB32 network byte order: rotate each pixel right
    // by one byte so [R, G, B, A] becomes [A, R, G, B].
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
