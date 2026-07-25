//! Desktop colour-scheme preference, read from the freedesktop
//! appearance portal.
//!
//! Tauri's own Linux theme detection reports the webview's colour
//! scheme, which on KDE Plasma Wayland frequently answers Light — or
//! errors — on a demonstrably dark desktop. That is why the tray used
//! to guess, and why the GUI's "Auto" mode could only approximate the
//! desktop with a `prefers-color-scheme` media query.
//!
//! `org.freedesktop.portal.Settings` is the authoritative source, and
//! it both answers on demand and signals changes, so one subscription
//! serves the tray and the webview alike.

use futures_util::StreamExt as _;
use tauri::{AppHandle, Emitter};
use tracing::debug;
use zbus::zvariant::Value;
use zbus::{Connection, Proxy};

/// Tauri event carrying the desktop's colour-scheme preference.
/// Payload is one of `"dark"`, `"light"`, `"no-preference"`.
pub(crate) const EVENT_COLOR_SCHEME_CHANGED: &str = "ludex:color-scheme-changed";

const PORTAL_SERVICE: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_INTERFACE: &str = "org.freedesktop.portal.Settings";
const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME_KEY: &str = "color-scheme";

/// What the desktop asks for. Mirrors the portal's `color-scheme`
/// enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorScheme {
    /// The desktop expresses no preference; the app decides.
    NoPreference,
    /// Prefer a dark appearance.
    Dark,
    /// Prefer a light appearance.
    Light,
}

impl ColorScheme {
    /// Wire form handed to the webview.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NoPreference => "no-preference",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Whether to paint dark. `NoPreference` resolves to dark, which
    /// matches the bundled tray icon and the usual Plasma panel.
    pub(crate) const fn prefers_dark(self) -> bool {
        !matches!(self, Self::Light)
    }

    /// Parse the event payload back into a scheme. Tauri delivers it
    /// JSON-encoded, so the quotes come along with it.
    pub(crate) fn from_wire(payload: &str) -> Option<Self> {
        match payload.trim().trim_matches('"') {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "no-preference" => Some(Self::NoPreference),
            _ => None,
        }
    }
}

/// Decode the portal's `color-scheme` value.
///
/// The enumeration is defined by the portal: `0` no preference, `1`
/// prefer dark, `2` prefer light. Unknown values are treated as no
/// preference, as the spec requires — a future variant must not be
/// read as one of the ones we happen to know.
///
/// Accepts a nested variant because the two portal methods disagree
/// about the shape: `ReadOne` returns the value itself, while the
/// older `Read` wraps it in a second variant.
pub(crate) fn scheme_from_value(value: &Value<'_>) -> Option<ColorScheme> {
    match value {
        Value::U32(n) => Some(match n {
            1 => ColorScheme::Dark,
            2 => ColorScheme::Light,
            _ => ColorScheme::NoPreference,
        }),
        Value::Value(inner) => scheme_from_value(inner),
        _ => None,
    }
}

/// Ask the portal for the current preference.
///
/// Tries `ReadOne` first and falls back to `Read`: the former was
/// added in version 2 of the interface, and a host running an older
/// portal answers only the latter.
async fn read_color_scheme(conn: &Connection) -> Option<ColorScheme> {
    for method in ["ReadOne", "Read"] {
        let reply = conn
            .call_method(
                Some(PORTAL_SERVICE),
                PORTAL_PATH,
                Some(PORTAL_INTERFACE),
                method,
                &(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY),
            )
            .await;
        let Ok(reply) = reply else { continue };
        // Bind the body: the decoded `Value` borrows from it.
        let body = reply.body();
        let Ok(value) = body.deserialize::<Value<'_>>() else {
            continue;
        };
        if let Some(scheme) = scheme_from_value(&value) {
            return Some(scheme);
        }
    }
    None
}

/// Current preference, or `None` when no portal is reachable.
pub(crate) async fn current_color_scheme() -> Option<ColorScheme> {
    let conn = Connection::session().await.ok()?;
    read_color_scheme(&conn).await
}

/// Watch the portal for colour-scheme changes and re-emit them as
/// Tauri events until the app exits.
///
/// Returns without doing anything if the session bus or the portal is
/// unreachable — a desktop with no portal is a supported
/// configuration, and the frontend falls back to its media query.
pub(crate) async fn run_appearance_watcher(app: AppHandle) {
    let Ok(conn) = Connection::session().await else {
        debug!("appearance: no session bus; Auto falls back to the media query");
        return;
    };

    // Subscribe *before* the initial read: a change landing between
    // the two would otherwise be dropped, and nothing reconciles it.
    let proxy = match Proxy::new(&conn, PORTAL_SERVICE, PORTAL_PATH, PORTAL_INTERFACE).await {
        Ok(proxy) => proxy,
        Err(e) => {
            debug!(error = %e, "appearance: no settings portal; falling back");
            return;
        }
    };

    let mut stream = match proxy.receive_signal("SettingChanged").await {
        Ok(stream) => stream,
        Err(e) => {
            debug!(error = %e, "appearance: could not watch SettingChanged");
            return;
        }
    };

    // Then broadcast the current value, so a webview that started
    // before this task isn't left waiting for the first change.
    if let Some(scheme) = read_color_scheme(&conn).await {
        let _ = app.emit(EVENT_COLOR_SCHEME_CHANGED, scheme.as_str());
    }

    while let Some(message) = stream.next().await {
        let body = message.body();
        let Ok((namespace, key, value)) = body.deserialize::<(String, String, Value<'_>)>() else {
            continue;
        };
        if namespace != APPEARANCE_NAMESPACE || key != COLOR_SCHEME_KEY {
            continue;
        }
        let Some(scheme) = scheme_from_value(&value) else {
            continue;
        };
        debug!(
            scheme = scheme.as_str(),
            "appearance: colour scheme changed"
        );
        let _ = app.emit(EVENT_COLOR_SCHEME_CHANGED, scheme.as_str());
    }
}

/// `invoke('get_color_scheme')` — the desktop's current preference, or
/// `"unavailable"` when no portal answered. The frontend falls back to
/// `prefers-color-scheme` on that value.
#[tauri::command]
pub(crate) async fn get_color_scheme() -> String {
    current_color_scheme()
        .await
        .map_or_else(|| "unavailable".to_owned(), |s| s.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_portal_enumeration() {
        assert_eq!(
            scheme_from_value(&Value::U32(0)),
            Some(ColorScheme::NoPreference)
        );
        assert_eq!(scheme_from_value(&Value::U32(1)), Some(ColorScheme::Dark));
        assert_eq!(scheme_from_value(&Value::U32(2)), Some(ColorScheme::Light));
    }

    // The portal spec reserves further values; guessing at one would
    // be worse than admitting no preference.
    #[test]
    fn treats_an_unknown_value_as_no_preference() {
        assert_eq!(
            scheme_from_value(&Value::U32(99)),
            Some(ColorScheme::NoPreference)
        );
    }

    // `Read` wraps the value in a second variant where `ReadOne`
    // does not; both have to decode or the fallback path is useless.
    #[test]
    fn unwraps_the_nested_variant_the_older_read_returns() {
        let nested = Value::Value(Box::new(Value::U32(1)));
        assert_eq!(scheme_from_value(&nested), Some(ColorScheme::Dark));
    }

    #[test]
    fn rejects_a_value_of_the_wrong_type() {
        assert_eq!(scheme_from_value(&Value::Str("dark".into())), None);
    }

    // Only an explicit light preference should light the UI; the tray
    // has always defaulted to dark when detection was inconclusive.
    // The tray reads this back off a Tauri event, where the payload
    // arrives JSON-encoded rather than bare.
    #[test]
    fn parses_the_json_encoded_event_payload() {
        assert_eq!(ColorScheme::from_wire("\"dark\""), Some(ColorScheme::Dark));
        assert_eq!(
            ColorScheme::from_wire("\"light\""),
            Some(ColorScheme::Light)
        );
        assert_eq!(
            ColorScheme::from_wire("\"no-preference\""),
            Some(ColorScheme::NoPreference)
        );
        assert_eq!(ColorScheme::from_wire("\"nonsense\""), None);
        assert_eq!(ColorScheme::from_wire(""), None);
    }

    #[test]
    fn no_preference_resolves_to_dark() {
        assert!(ColorScheme::NoPreference.prefers_dark());
        assert!(ColorScheme::Dark.prefers_dark());
        assert!(!ColorScheme::Light.prefers_dark());
    }
}
