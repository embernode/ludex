//! GOG `goggame-*.info` enricher.
//!
//! GOG Galaxy and Heroic-managed installs drop a
//! `goggame-<numericId>.info` JSON file into the game's install directory
//! containing (at least) the canonical `name`. For Proton games the
//! `.info` may live a directory or two above the actual exe (Heroic
//! nests the Windows build under a `drive_c/...` wineprefix path), so
//! we walk up to three parent directories looking for one.

use std::path::Path;

use ludex_core::{Application, IdentityUpdate};
use serde::Deserialize;
use tracing::debug;

use crate::context::EnrichmentContext;

/// How far up the directory tree to search for a `goggame-*.info` file.
const PARENT_WALK_DEPTH: usize = 3;

/// Shape of the fields we read. Everything else in the file is ignored,
/// so a schema change that adds or removes peripheral fields cannot
/// break us.
#[derive(Debug, Deserialize)]
struct GogInfo {
    name: Option<String>,
    version: Option<serde_json::Value>,
}

/// Enrich an application from a nearby `goggame-*.info` file.
pub async fn enrich(app: &Application, _ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    let exe = app.executable_path.as_ref()?;
    let start = Path::new(exe).parent()?;

    let mut cursor: &Path = start;
    for _ in 0..PARENT_WALK_DEPTH {
        if let Some(update) = try_directory(cursor).await {
            return Some(update);
        }
        match cursor.parent() {
            Some(p) => cursor = p,
            None => break,
        }
    }
    None
}

async fn try_directory(dir: &Path) -> Option<IdentityUpdate> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(file_name) = name.to_str() else {
            continue;
        };
        if !file_name.starts_with("goggame-")
            || !Path::new(file_name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("info"))
        {
            continue;
        }
        let bytes = tokio::fs::read(entry.path()).await.ok()?;
        let info: GogInfo = serde_json::from_slice(&bytes).ok()?;
        let product_name = info.name?;
        if product_name.trim().is_empty() {
            continue;
        }
        debug!(path = %entry.path().display(), %product_name, "gog enricher matched");
        return Some(IdentityUpdate {
            product_name: Some(product_name),
            version: info.version.and_then(value_to_version_string),
            ..Default::default()
        });
    }
    None
}

/// GOG's `version` field has been observed as a string, an integer, or a
/// JSON number representing a build id. Coerce to `String` when we can,
/// and drop it if the shape is unrecognised rather than emitting
/// something misleading.
fn value_to_version_string(v: serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_coercion_handles_common_shapes() {
        assert_eq!(
            value_to_version_string(serde_json::json!("1.2.3")).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            value_to_version_string(serde_json::json!(42)).as_deref(),
            Some("42")
        );
        assert_eq!(value_to_version_string(serde_json::json!("")), None);
        assert_eq!(value_to_version_string(serde_json::json!(null)), None);
        assert_eq!(value_to_version_string(serde_json::json!([1, 2])), None);
    }
}
