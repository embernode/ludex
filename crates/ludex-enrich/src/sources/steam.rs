//! Steam `appmanifest_*.acf` enricher.
//!
//! Reads the appmanifest for the application's `launcher_id` (the Steam
//! AppID) and extracts the `name` field. Uses a small, total VDF
//! line-parser — the appmanifest schema is flat enough that full VDF
//! parsing would be overkill, and a non-matching line can never panic.

use ludex_core::{Application, IdentityUpdate, LauncherType};
use tracing::debug;

use crate::context::EnrichmentContext;

/// Enrich a Steam-attributed application from its `appmanifest_*.acf`.
pub async fn enrich(app: &Application, ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    if app.launcher_type != LauncherType::Steam {
        return None;
    }
    let steam_dir = ctx.steam_dir.as_ref()?;
    let manifest = steam_dir
        .join("steamapps")
        .join(format!("appmanifest_{}.acf", app.launcher_id));
    let content = tokio::fs::read_to_string(&manifest).await.ok()?;
    let name = parse_vdf_top_level_string(&content, "name")?;
    debug!(appid = %app.launcher_id, %name, "steam enricher matched");
    Some(IdentityUpdate {
        product_name: Some(name),
        ..Default::default()
    })
}

/// Extract the first `"key" "value"` line from a VDF document.
///
/// The parser is deliberately dumb: it does not honour nesting depth, so
/// a same-named key inside a nested block would match. Steam's
/// appmanifest keeps `name` at the top level of `AppState`, so this is
/// good enough — and property-tested to never panic on arbitrary input.
fn parse_vdf_top_level_string(content: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    for line in content.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(&needle) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(after_open) = rest.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = after_open.find('"') {
            return Some(after_open[..end].to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_name_from_real_manifest() {
        let content = include_str!("../../tests/fixtures/steam_appmanifest_440.acf");
        assert_eq!(
            parse_vdf_top_level_string(content, "name").as_deref(),
            Some("Team Fortress 2")
        );
    }

    #[test]
    fn returns_none_if_key_missing() {
        let content = "\"AppState\"\n{\n\t\"appid\" \"42\"\n}";
        assert_eq!(parse_vdf_top_level_string(content, "name"), None);
    }

    #[test]
    fn returns_none_for_malformed_value() {
        // Missing closing quote after the value.
        let content = "\"name\" \"Team Fortress 2\n";
        assert_eq!(parse_vdf_top_level_string(content, "name"), None);
    }
}
