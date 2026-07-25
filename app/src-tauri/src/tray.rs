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
//! Icon follows the system colour scheme. Both inks are embedded at
//! compile time — black shapes for a light panel, white shapes for a
//! dark one — and the tray picks between them from the freedesktop
//! appearance portal, falling back to Tauri's `window.theme()` on
//! desktops that don't answer. The portal is authoritative because
//! `window.theme()` reports the webview's belief, which on KDE Plasma
//! Wayland frequently disagrees with the desktop.
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

use crate::appearance::{ColorScheme, EVENT_COLOR_SCHEME_CHANGED};
use crate::bridge::{
    TrackerBridge, EVENT_DAEMON_DISCONNECTED, EVENT_DAEMON_RECONNECTED, EVENT_SESSION_ENDED,
    EVENT_SESSION_STARTED,
};

const MAIN_WINDOW: &str = "main";
const TOOLTIP_IDLE: &str = "ludex";
/// Shown between `session-started` and the moment `GetApplication`
/// resolves the name — usually a single D-Bus round trip, but we
/// flash something rather than the idle text so the user sees the
/// session is being tracked.
const TOOLTIP_ACTIVE_UNRESOLVED: &str = "ludex · session active";

/// PNG bytes for the four icon variants (theme × session-active).
/// Kept at 256px so downsampling to typical tray sizes
/// (16/22/24) stays crisp. The active variant differs from the
/// idle one only in the inner Play triangle, which is filled
/// green to indicate an in-flight session — same overall logo
/// silhouette, so the user reads "active" as a colour change
/// rather than a different shape.
// The file names describe the colour of the artwork, not the panel it
// belongs on: `icon_light.png` is the *white* silhouette, which is the
// one that shows up against a dark panel. Naming the constants after
// the ink rather than the theme keeps that from being read backwards.
/// How long to wait for the appearance portal while seeding the icon.
/// A local bus call is a couple of milliseconds; this only bounds the
/// pathological case so a wedged portal can't delay the window.
const PORTAL_SEED_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

const ICON_PNG_WHITE: &[u8] = include_bytes!("../icons/icon_light.png");
const ICON_PNG_BLACK: &[u8] = include_bytes!("../icons/icon.png");
const ICON_PNG_ACTIVE_WHITE: &[u8] = include_bytes!("../icons/icon_active_light.png");
const ICON_PNG_ACTIVE_BLACK: &[u8] = include_bytes!("../icons/icon_active.png");

struct ThemeVariant {
    /// No session in flight; the plain logo silhouette.
    idle: Icon,
    /// Session active; play triangle filled green, rest of the
    /// logo unchanged so the cue is unambiguous on the panel.
    active: Icon,
}

struct ThemeIcons {
    /// Black shapes — the pair that reads against a light panel.
    on_light: ThemeVariant,
    /// White shapes — the pair that reads against a dark panel.
    on_dark: ThemeVariant,
}

struct LudexTray<R: Runtime> {
    app: AppHandle<R>,
    icons: ThemeIcons,
    is_dark: bool,
    /// True between `session-started` and `session-ended` events.
    /// Selects the active (green-play) icon variant in
    /// [`Self::icon_pixmap`]; the tooltip text already moves to
    /// the per-game string in parallel.
    is_active: bool,
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
        let variant = if self.is_dark {
            &self.icons.on_dark
        } else {
            &self.icons.on_light
        };
        let icon = if self.is_active {
            &variant.active
        } else {
            &variant.idle
        };
        vec![icon.clone()]
    }

    /// Plasma 6's StatusNotifierItem host caches the rendered icon
    /// keyed on `IconName`. With an empty name it caches the first
    /// `IconPixmap` it sees and ignores subsequent `NewIcon` signals
    /// for the lifetime of the SNI item, so updating `icon_pixmap`
    /// alone never repaints. Returning a state-dependent name flips
    /// that cache key on every idle ↔ active transition: Plasma
    /// looks the name up, fails to find it in the icon theme
    /// (intentional), and falls back to refetching `IconPixmap`,
    /// which we've just changed.
    fn icon_name(&self) -> String {
        if self.is_active {
            "ludex-active".into()
        } else {
            "ludex-idle".into()
        }
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
#[derive(Debug, PartialEq, Eq)]
enum TrayStateUpdate {
    TooltipStarted(i64),
    TooltipEnded,
    /// The bridge reconnected to the daemon. The tray reconciles its
    /// active state against the daemon rather than trusting that it
    /// caught the `SessionStarted` signal, which can be missed in the
    /// window between the daemon claiming its bus name and the bridge
    /// rebuilding its signal subscription.
    DaemonReconnected,
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
        on_light: ThemeVariant {
            idle: decode_icon(ICON_PNG_BLACK)?,
            active: decode_icon(ICON_PNG_ACTIVE_BLACK)?,
        },
        on_dark: ThemeVariant {
            idle: decode_icon(ICON_PNG_WHITE)?,
            active: decode_icon(ICON_PNG_ACTIVE_WHITE)?,
        },
    };

    // Initial theme, resolved *before* the tray is built: the handle
    // it would need to correct itself afterwards doesn't exist until
    // the service task has spawned, so a later fix-up would fire into
    // an empty slot and be dropped.
    //
    // The portal is authoritative. Asking it costs a couple of
    // milliseconds on the session bus, bounded below so a wedged
    // portal can't hold up the window.
    let portal_scheme = tauri::async_runtime::block_on(async {
        tokio::time::timeout(
            PORTAL_SEED_TIMEOUT,
            crate::appearance::current_color_scheme(),
        )
        .await
        .ok()
        .flatten()
    });

    let initial_is_dark = match portal_scheme {
        Some(scheme) => scheme.prefers_dark(),
        // No portal: fall back to the main window, which on KDE
        // Plasma Wayland frequently reports Light or errors on a
        // demonstrably dark system. Defaulting to dark when detection
        // is inconclusive matches the embedded bundle icon and the
        // usual Plasma panel.
        None => match app
            .get_webview_window(MAIN_WINDOW)
            .and_then(|w| w.theme().ok())
        {
            Some(Theme::Light) => false,
            // `Some(Theme::Dark)` → dark, `None`/`Err` → default dark
            // (the Theme enum is #[non_exhaustive], so the wildcard
            // also covers any variant added upstream).
            _ => true,
        },
    };

    let tray = LudexTray {
        app: app.clone(),
        icons,
        is_dark: initial_is_dark,
        is_active: false,
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

    register_event_listeners(app, tx);

    Ok(())
}

/// Register the Tauri event listeners that translate daemon lifecycle
/// events into [`TrayStateUpdate`]s on the worker channel. Split out of
/// [`install`] so the event→update wiring can be unit-tested without a
/// window or a StatusNotifierItem service.
///
/// Payload shape: `session-started` carries a JSON-encoded `i64` (just
/// the number); `session-ended` carries a JSON object but we only need
/// the transition, not the id. `daemon-disconnected` carries no useful
/// payload — a daemon killed mid-session never sends `session-ended`, so
/// without this the tray would keep its green (active) icon with nothing
/// behind it; treat the disconnect as a session end and fall back to
/// idle. A genuinely-running session re-announces via `session-started`
/// when the bridge reconnects (the daemon re-emits it at cold start).
fn register_event_listeners<R: Runtime>(
    app: &AppHandle<R>,
    tx: mpsc::UnboundedSender<TrayStateUpdate>,
) {
    let tx_started = tx.clone();
    app.listen_any(EVENT_SESSION_STARTED, move |event| {
        let Ok(id) = event.payload().parse::<i64>() else {
            return;
        };
        let _ = tx_started.send(TrayStateUpdate::TooltipStarted(id));
    });
    let tx_ended = tx.clone();
    app.listen_any(EVENT_SESSION_ENDED, move |_event| {
        let _ = tx_ended.send(TrayStateUpdate::TooltipEnded);
    });
    let tx_disconnected = tx.clone();
    app.listen_any(EVENT_DAEMON_DISCONNECTED, move |_event| {
        let _ = tx_disconnected.send(TrayStateUpdate::TooltipEnded);
    });
    // On reconnect, reconcile rather than assume idle: a game may have
    // been running the whole time (or the daemon just re-detected one at
    // cold start) and its SessionStarted can land before we re-subscribe.
    let tx_reconnected = tx.clone();
    app.listen_any(EVENT_DAEMON_RECONNECTED, move |_event| {
        let _ = tx_reconnected.send(TrayStateUpdate::DaemonReconnected);
    });
    // The portal is authoritative for the desktop's colour scheme,
    // unlike the webview's own report, so let it override whatever
    // the seed above guessed.
    let tx_scheme = tx;
    app.listen_any(EVENT_COLOR_SCHEME_CHANGED, move |event| {
        if let Some(scheme) = ColorScheme::from_wire(event.payload()) {
            let _ = tx_scheme.send(TrayStateUpdate::ThemeChanged(scheme.prefers_dark()));
        }
    });
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
                // Flip the active flag first so the icon swap
                // takes the same `update()` round-trip as the
                // unresolved tooltip — fewer paints than two
                // separate calls.
                apply_session(&handle_slot, true, TOOLTIP_ACTIVE_UNRESOLVED.to_owned()).await;
                let name = resolve_app_name(&bridge, id).await;
                let text = match name {
                    Some(n) if !n.trim().is_empty() => format!("ludex · {n}"),
                    _ => TOOLTIP_ACTIVE_UNRESOLVED.to_owned(),
                };
                apply_tooltip(&handle_slot, text).await;
            }
            TrayStateUpdate::TooltipEnded => {
                apply_session(&handle_slot, false, TOOLTIP_IDLE.to_owned()).await;
            }
            TrayStateUpdate::DaemonReconnected => {
                // Reconcile against the daemon's current state instead of
                // waiting for the next session boundary. If a game is
                // being tracked, go (back) to active with its name;
                // otherwise settle on idle.
                match active_session(&bridge).await {
                    Some(name) => {
                        let text = if name.trim().is_empty() {
                            TOOLTIP_ACTIVE_UNRESOLVED.to_owned()
                        } else {
                            format!("ludex · {name}")
                        };
                        apply_session(&handle_slot, true, text).await;
                    }
                    None => apply_session(&handle_slot, false, TOOLTIP_IDLE.to_owned()).await,
                }
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

/// Ask the daemon whether a game is being tracked right now, returning
/// its product name (possibly empty) if so. The newest session is open
/// exactly when its `ended_at` is empty; `list_recent_sessions` orders by
/// start time and includes open sessions, so the first row is the live
/// one. Used to reconcile the tray after a reconnect without depending on
/// a `SessionStarted` signal that may have been missed.
///
/// Best-effort: any proxy/RPC failure returns `None`, so a transient
/// error at reconnect settles the tray on idle rather than green. That
/// only re-opens the original stuck-idle window until the next
/// `SessionStarted` (the subscription is live again by this point), so it
/// degrades to the pre-fix behaviour rather than to something worse.
async fn active_session(bridge: &Arc<TrackerBridge>) -> Option<String> {
    let proxy = match bridge.proxy().await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(error = %e, "tray worker: bridge proxy unavailable for reconcile");
            return None;
        }
    };
    let recent = match proxy.list_recent_sessions(1).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "tray worker: list_recent_sessions failed");
            return None;
        }
    };
    let top = recent.into_iter().next()?;
    top.ended_at.trim().is_empty().then_some(top.product_name)
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

async fn apply_theme<R: Runtime>(handle_slot: &Arc<OnceLock<Handle<LudexTray<R>>>>, is_dark: bool) {
    let Some(handle) = handle_slot.get().cloned() else {
        return;
    };
    handle
        .update(move |t: &mut LudexTray<R>| {
            t.is_dark = is_dark;
        })
        .await;
}

/// Combined active-flag + tooltip update. Both fields move on
/// the same lifecycle event (session start / end), so bundling
/// them into one `Handle::update` call costs one ksni redraw
/// instead of two.
async fn apply_session<R: Runtime>(
    handle_slot: &Arc<OnceLock<Handle<LudexTray<R>>>>,
    is_active: bool,
    tooltip_title: String,
) {
    let Some(handle) = handle_slot.get().cloned() else {
        return;
    };
    handle
        .update(move |t: &mut LudexTray<R>| {
            t.is_active = is_active;
            t.tooltip_title = tooltip_title;
        })
        .await;
}

fn apply_window_icon<R: Runtime>(window: &tauri::WebviewWindow<R>, is_dark: bool) {
    let bytes = if is_dark {
        ICON_PNG_WHITE
    } else {
        ICON_PNG_BLACK
    };
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
        // KDE Plasma 6 Wayland: after a `hide()` → `show()`
        // cycle the server-side decoration is *drawn* but
        // doesn't receive pointer events — the titlebar's
        // min/max/close buttons read as dead. KWin re-binds
        // the decoration's input grab when the toplevel
        // receives a `configure` event; `show()` alone doesn't
        // generate one, but a state change like maximise
        // does (which is why double-clicking the titlebar
        // empirically fixes the bug). Toggling `resizable`
        // produces the same configure round-trip without any
        // visible size change. Idempotent and harmless when
        // the bug isn't present.
        if let Ok(resizable) = window.is_resizable() {
            let _ = window.set_resizable(!resizable);
            let _ = window.set_resizable(resizable);
        }
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
        show_main(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::mock_app;
    use tauri::Emitter;

    /// Register the listeners on a headless mock app, emit `event` with
    /// `payload`, and return whatever `TrayStateUpdate` (if any) landed
    /// on the channel. Tauri invokes Rust-side `listen_any` handlers
    /// synchronously within `emit`, so a `try_recv` right after observes
    /// the result without spinning.
    fn update_for(event: &str, payload: impl serde::Serialize + Clone) -> Option<TrayStateUpdate> {
        let app = mock_app();
        let (tx, mut rx) = mpsc::unbounded_channel::<TrayStateUpdate>();
        register_event_listeners(app.handle(), tx);
        app.handle().emit(event, payload).unwrap();
        rx.try_recv().ok()
    }

    // The payload arrives JSON-encoded through Tauri's event system,
    // so this also pins `ColorScheme::from_wire`'s quote handling.
    #[test]
    fn color_scheme_event_maps_to_theme_changed() {
        assert_eq!(
            update_for(EVENT_COLOR_SCHEME_CHANGED, "dark"),
            Some(TrayStateUpdate::ThemeChanged(true)),
        );
        assert_eq!(
            update_for(EVENT_COLOR_SCHEME_CHANGED, "light"),
            Some(TrayStateUpdate::ThemeChanged(false)),
        );
        // The desktop declining to choose still needs an icon; dark
        // matches the bundled default and the usual Plasma panel.
        assert_eq!(
            update_for(EVENT_COLOR_SCHEME_CHANGED, "no-preference"),
            Some(TrayStateUpdate::ThemeChanged(true)),
        );
    }

    #[test]
    fn unparseable_color_scheme_payload_is_ignored() {
        assert_eq!(update_for(EVENT_COLOR_SCHEME_CHANGED, "nonsense"), None);
    }

    #[test]
    fn session_started_maps_to_tooltip_started_with_id() {
        assert_eq!(
            update_for(EVENT_SESSION_STARTED, 42_i64),
            Some(TrayStateUpdate::TooltipStarted(42)),
        );
    }

    #[test]
    fn session_ended_maps_to_tooltip_ended() {
        assert_eq!(
            update_for(EVENT_SESSION_ENDED, ()),
            Some(TrayStateUpdate::TooltipEnded),
        );
    }

    /// The bug this fixes: a daemon killed mid-session never emits
    /// `session-ended`, so the tray kept its green (active) icon. The
    /// bridge's `daemon-disconnected` event must drive the tray back to
    /// idle. Without the disconnect listener this returns `None`.
    #[test]
    fn daemon_disconnected_resets_tray_to_idle() {
        assert_eq!(
            update_for(EVENT_DAEMON_DISCONNECTED, ()),
            Some(TrayStateUpdate::TooltipEnded),
        );
    }

    /// On reconnect the tray must reconcile (query the daemon), not blindly
    /// reset — a game may still be running. The listener routes the bridge's
    /// reconnect event to the reconcile update.
    #[test]
    fn daemon_reconnected_maps_to_reconcile() {
        assert_eq!(
            update_for(EVENT_DAEMON_RECONNECTED, ()),
            Some(TrayStateUpdate::DaemonReconnected),
        );
    }
}
