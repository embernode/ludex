//! Steam `appmanifest_*.acf` enricher.
//!
//! Reads the appmanifest for the application's `launcher_id` (the Steam
//! AppID) and extracts the `name` field. The VDF parsing itself lives
//! in [`ludex_core::vdf`] so the daemon and the enrich crate share one
//! implementation — appmanifest schemas are flat enough that a
//! full-grammar VDF parser would be overkill.

use ludex_core::vdf;
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
    let name = vdf::parse_top_level_string(&content, "name")?;
    debug!(appid = %app.launcher_id, %name, "steam enricher matched");
    Some(IdentityUpdate {
        product_name: Some(name),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use ludex_core::vdf;

    #[test]
    fn extracts_name_from_real_manifest() {
        let content = include_str!("../../tests/fixtures/steam_appmanifest_440.acf");
        assert_eq!(
            vdf::parse_top_level_string(content, "name").as_deref(),
            Some("Team Fortress 2")
        );
    }
}
