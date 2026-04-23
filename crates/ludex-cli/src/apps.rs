//! `ludex apps` subcommands.
//!
//! Reads the `applications` table directly — no daemon D-Bus hop —
//! so the list is available even when `ludex-daemon` isn't running.
//! Primary use is looking up the numeric id a `ludex merge` call
//! needs when Tauri's webview hides the `/app/<id>` URL.

use anyhow::{Context, Result};
use ludex_core::{default_database_path, Database};
use time::format_description::FormatItem;
use time::macros::format_description;

const LAST_PLAYED_FORMAT: &[FormatItem<'_>] = format_description!("[year]-[month]-[day]");

/// Print every tracked application, most-recently-played first.
/// Columns: id, launcher:id, product name, run count, last played.
pub(crate) async fn list() -> Result<()> {
    let db_path = default_database_path().context("neither XDG_DATA_HOME nor HOME is set")?;
    if !db_path.exists() {
        eprintln!(
            "no database at {} — has ludex-daemon run yet?",
            db_path.display()
        );
        return Ok(());
    }
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("open database at {}", db_path.display()))?;
    let apps = db
        .applications()
        .list()
        .await
        .context("list applications")?;
    db.close().await;

    if apps.is_empty() {
        println!("(no applications tracked yet)");
        return Ok(());
    }

    let id_width = apps
        .iter()
        .map(|a| a.id.to_string().len())
        .max()
        .unwrap_or(2)
        .max(2);
    let launcher_width = apps
        .iter()
        .map(|a| a.launcher_type.to_string().len() + 1 + a.launcher_id.chars().count())
        .max()
        .unwrap_or(16)
        .min(48); // don't let one pathological Steam-path blow the table out
    let name_width = apps
        .iter()
        .map(|a| a.product_name.chars().count())
        .max()
        .unwrap_or(16)
        .clamp(8, 40);

    println!(
        "{:>id_width$}  {:<launcher_width$}  {:<name_width$}  {:>6}  last played",
        "id", "launcher", "application", "runs",
    );
    println!(
        "{}",
        "─".repeat(id_width + 2 + launcher_width + 2 + name_width + 2 + 6 + 2 + 10)
    );
    for app in &apps {
        let launcher = format!("{}:{}", app.launcher_type, app.launcher_id);
        let launcher = truncate(&launcher, launcher_width);
        let name = truncate(&app.product_name, name_width);
        let last = app
            .last_played_at
            .and_then(|t| t.format(LAST_PLAYED_FORMAT).ok())
            .unwrap_or_else(|| "never".to_owned());
        println!(
            "{id:>id_width$}  {launcher:<launcher_width$}  {name:<name_width$}  {runs:>6}  {last}",
            id = app.id,
            runs = app.stat_run_count,
        );
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_owned()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}
