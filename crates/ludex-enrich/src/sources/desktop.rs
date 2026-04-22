//! `.desktop` file enricher.
//!
//! For Flatpak applications we look up `<app-id>.desktop` directly. For
//! native applications with a known executable path we scan the XDG
//! application directories and pick the first `.desktop` whose `Exec=`
//! field resolves to that executable.
//!
//! The Steam case is intentionally not handled here: Steam writes
//! `steam_app_<appid>.desktop` under Flatpak but not every distro, and
//! the authoritative name for Steam games comes from `appmanifest_*.acf`
//! (the Steam enricher), so duplicating the logic here would just
//! invite divergence.

use std::path::{Path, PathBuf};

use freedesktop_desktop_entry::DesktopEntry;
use ludex_core::{Application, IdentityUpdate, LauncherType};
use tracing::debug;

use crate::context::EnrichmentContext;

/// Enrich an application from the best-matching `.desktop` entry.
pub async fn enrich(app: &Application, ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    match app.launcher_type {
        LauncherType::Flatpak => enrich_flatpak(app, ctx).await,
        LauncherType::Native => enrich_native(app, ctx).await,
        // Steam, Lutris, Heroic have authoritative launchers elsewhere;
        // their .desktop files can be stale or absent, so we skip them.
        _ => None,
    }
}

async fn enrich_flatpak(app: &Application, ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    let target_filename = format!("{}.desktop", app.launcher_id);
    for dir in &ctx.desktop_dirs {
        let candidate = dir.join(&target_filename);
        if let Some(u) = try_desktop_file(&candidate).await {
            return Some(u);
        }
    }
    None
}

async fn enrich_native(app: &Application, ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    let exe = app.executable_path.as_ref()?;
    let exe_path = Path::new(exe);
    for dir in &ctx.desktop_dirs {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "desktop")
                && desktop_exec_matches(&path, exe_path).await
            {
                if let Some(u) = try_desktop_file(&path).await {
                    return Some(u);
                }
            }
        }
    }
    None
}

/// Returns `true` if the `.desktop` file's `Exec=` (or `TryExec=`) resolves
/// to `target_exe`. Matching is on canonicalised paths so symlink farms
/// like `/usr/bin/ludex -> /usr/lib/ludex/ludex` behave correctly.
async fn desktop_exec_matches(desktop: &Path, target_exe: &Path) -> bool {
    let Ok(bytes) = tokio::fs::read_to_string(desktop).await else {
        return false;
    };
    let Ok(entry) = DesktopEntry::from_str(desktop, &bytes, None::<&[&str]>) else {
        return false;
    };
    let candidate_exec = entry.exec().or_else(|| entry.try_exec());
    let Some(exec_field) = candidate_exec else {
        return false;
    };
    // Exec= may carry arguments and field codes (%U, %f, etc.); take the
    // first token only.
    let Some(first_token) = exec_field.split_whitespace().next() else {
        return false;
    };
    let candidate = PathBuf::from(first_token);
    paths_canonically_equal(&candidate, target_exe).await
}

async fn paths_canonically_equal(a: &Path, b: &Path) -> bool {
    let ac = tokio::fs::canonicalize(a)
        .await
        .unwrap_or_else(|_| a.to_path_buf());
    let bc = tokio::fs::canonicalize(b)
        .await
        .unwrap_or_else(|_| b.to_path_buf());
    ac == bc
}

async fn try_desktop_file(path: &Path) -> Option<IdentityUpdate> {
    let bytes = tokio::fs::read_to_string(path).await.ok()?;
    let entry = DesktopEntry::from_str(path, &bytes, None::<&[&str]>).ok()?;
    let name = entry
        .name(&[] as &[&str])
        .map(std::borrow::Cow::into_owned)?;
    debug!(path = %path.display(), %name, "desktop enricher matched");
    Some(IdentityUpdate {
        product_name: Some(name),
        ..Default::default()
    })
}
