//! `ludex merge` subcommand.
//!
//! Folds one application row into another. The session history,
//! aggregate stats, first-seen / last-played range, and any
//! not-yet-populated metadata slots on the destination are carried
//! over; the source row is deleted. Intended for post-migration
//! deduplication — for example, a game the Steam source already
//! tracks as `(steam, <appid>)` colliding with the same game
//! brought in under `(native, <exe_path>)` by a legacy importer.

use anyhow::{Context, Result};
use ludex_core::{default_database_path, Database};

/// Refuse to run while the daemon owns its D-Bus name — merging
/// moves sessions and rewrites aggregates, which would race with
/// a live session manager holding src's id in memory.
pub(crate) async fn run(src_id: i64, dst_id: i64) -> Result<()> {
    if daemon_active().await {
        anyhow::bail!(
            "ludex-daemon is active — stop it before merging \
             (e.g. `systemctl --user stop ludex-daemon`)"
        );
    }
    let db_path = default_database_path().context("neither XDG_DATA_HOME nor HOME is set")?;
    if !db_path.exists() {
        anyhow::bail!("no ludex database at {}", db_path.display());
    }
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("open database at {}", db_path.display()))?;

    // Capture both labels before the merge so the summary can
    // report what actually collapsed — after merge_into, src is
    // gone and dst carries its own product name regardless of what
    // src was called.
    let src = db
        .applications()
        .find_by_id(src_id)
        .await?
        .with_context(|| format!("application id {src_id} not found"))?;
    let dst = db
        .applications()
        .find_by_id(dst_id)
        .await?
        .with_context(|| format!("application id {dst_id} not found"))?;

    db.applications()
        .merge_into(src_id, dst_id)
        .await
        .context("merge")?;
    db.close().await;

    println!(
        "merged {} (id={}, {}:{}) → {} (id={}, {}:{})",
        src.product_name,
        src.id,
        src.launcher_type,
        src.launcher_id,
        dst.product_name,
        dst.id,
        dst.launcher_type,
        dst.launcher_id,
    );
    Ok(())
}

/// Copied from `backup.rs` — kept duplicated for now. If a third
/// CLI subcommand needs the same check we'll factor it out.
async fn daemon_active() -> bool {
    let Ok(conn) = zbus::Connection::session().await else {
        return false;
    };
    let Ok(proxy) = zbus::fdo::DBusProxy::new(&conn).await else {
        return false;
    };
    let Ok(names) = proxy.list_names().await else {
        return false;
    };
    names.iter().any(|n| n.as_str() == "net.ludex.Tracker1")
}
