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
//! We use [`ksni`] rather than Tauri's built-in `tray-icon` because
//! the latter pulls in the abandoned `libappindicator-rs` crate
//! which wraps the deprecated `libayatana-appindicator` C library.
//! `ksni` is a pure-Rust implementation of the StatusNotifierItem
//! spec and talks directly to the D-Bus host, so no C dependency
//! is involved.
//!
//! The Tauri listener callback is synchronous while the name-
//! resolution RPC is async, so the listener only pushes a
//! [`TooltipUpdate`] onto an unbounded channel — the worker task
//! owned by this module picks it up, calls the bridge's proxy, and
//! updates the tray via the ksni [`Handle`].

use std::sync::{Arc, OnceLock};

use ksni::menu::StandardItem;
use ksni::{Handle, Icon, MenuItem, ToolTip, Tray, TrayMethods};
use tauri::{AppHandle, Listener, Manager, Runtime, WindowEvent};
use tokio::sync::mpsc;

use crate::bridge::{TrackerBridge, EVENT_SESSION_ENDED, EVENT_SESSION_STARTED};

const MAIN_WINDOW: &str = "main";
const TOOLTIP_IDLE: &str = "ludex";
/// Shown between `session-started` and the moment `GetApplication`
/// resolves the name — usually a single D-Bus round trip, but we
/// flash something rather than the idle text so the user sees the
/// session is being tracked.
const TOOLTIP_ACTIVE_UNRESOLVED: &str = "ludex · session active";

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

/// Message pushed by the D-Bus event listeners onto the tooltip
/// worker's channel. Synchronous listeners can't await the name
/// lookup themselves, so they delegate via this.
#[derive(Debug)]
enum TooltipUpdate {
    Started(i64),
    Ended,
}

/// Spawn the StatusNotifierItem service, install the close-to-tray
/// hook on the main window, spawn the tooltip worker, and register
/// listeners that push session events onto the worker's channel.
pub(crate) fn install<R: Runtime>(
    app: &AppHandle<R>,
    bridge: Arc<TrackerBridge>,
) -> anyhow::Result<()> {
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
    // is up. The tooltip worker and the listeners both share the
    // same OnceLock — they see the handle as soon as the spawn
    // finishes, and no-op before then.
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

    // The worker owns the bridge handle and serialises tooltip
    // updates, so rapid session-started / session-ended events stay
    // in order even though the D-Bus round-trips are async.
    let (tooltip_tx, tooltip_rx) = mpsc::unbounded_channel::<TooltipUpdate>();
    let handle_slot_for_worker = Arc::clone(&handle_slot);
    tauri::async_runtime::spawn(async move {
        run_tooltip_worker::<R>(tooltip_rx, handle_slot_for_worker, bridge).await;
    });

    // Payload shape: session-started carries a JSON-encoded i64
    // (just the number as a string); session-ended carries a JSON
    // object but we only need the transition, not the id.
    let tx_started = tooltip_tx.clone();
    app.listen_any(EVENT_SESSION_STARTED, move |event| {
        let Ok(id) = event.payload().parse::<i64>() else {
            return;
        };
        let _ = tx_started.send(TooltipUpdate::Started(id));
    });
    let tx_ended = tooltip_tx;
    app.listen_any(EVENT_SESSION_ENDED, move |_event| {
        let _ = tx_ended.send(TooltipUpdate::Ended);
    });

    Ok(())
}

/// Drain [`TooltipUpdate`]s from the listener channel. On every
/// `Started(id)` we optimistically set the unresolved tooltip so the
/// user has feedback immediately, then look up the name and land
/// the final text. On `Ended` we reset to idle.
async fn run_tooltip_worker<R: Runtime>(
    mut rx: mpsc::UnboundedReceiver<TooltipUpdate>,
    handle_slot: Arc<OnceLock<Handle<LudexTray<R>>>>,
    bridge: Arc<TrackerBridge>,
) {
    while let Some(update) = rx.recv().await {
        match update {
            TooltipUpdate::Started(id) => {
                apply_tooltip(&handle_slot, TOOLTIP_ACTIVE_UNRESOLVED.to_owned()).await;
                let name = resolve_app_name(&bridge, id).await;
                let text = match name {
                    Some(n) if !n.trim().is_empty() => format!("ludex · {n}"),
                    _ => TOOLTIP_ACTIVE_UNRESOLVED.to_owned(),
                };
                apply_tooltip(&handle_slot, text).await;
            }
            TooltipUpdate::Ended => {
                apply_tooltip(&handle_slot, TOOLTIP_IDLE.to_owned()).await;
            }
        }
    }
}

async fn resolve_app_name(bridge: &Arc<TrackerBridge>, id: i64) -> Option<String> {
    let proxy = match bridge.proxy().await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(error = %e, "tooltip worker: bridge proxy unavailable");
            return None;
        }
    };
    match proxy.get_application(id).await {
        Ok(apps) => apps.into_iter().next().map(|a| a.product_name),
        Err(e) => {
            tracing::debug!(error = %e, id, "tooltip worker: GetApplication failed");
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
