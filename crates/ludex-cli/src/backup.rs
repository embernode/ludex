//! `ludex backup` subcommands.
//!
//! Writes go through `ludex_core::backup::snapshot_now`, which is
//! the same primitive the daemon's scheduler uses — one code path
//! for periodic and manual snapshots keeps retention behaviour
//! consistent.

use anyhow::{Context, Result};
use ludex_core::{backup::snapshot_now, default_database_path, Database};

/// Take one snapshot now and prune to the configured retention.
pub(crate) async fn now() -> Result<()> {
    let db_path = default_database_path().context("neither XDG_DATA_HOME nor HOME is set")?;
    if !db_path.exists() {
        eprintln!(
            "no database at {} — has ludex-daemon run yet?",
            db_path.display()
        );
        return Ok(());
    }
    // Open in read-only-ish fashion: we still need a pool to run
    // VACUUM INTO, but a concurrent daemon holds its own pool on
    // the same file (WAL supports that). The busy_timeout in
    // `Database::open` handles the brief contention window.
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("open database at {}", db_path.display()))?;
    let dst = snapshot_now(&db, None).await.context("snapshot")?;
    db.close().await;
    println!("{}", dst.display());
    Ok(())
}
