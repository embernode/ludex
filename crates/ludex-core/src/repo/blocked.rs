//! Persistence for the user's blocked-applications list.
//!
//! A game whose `(launcher_type, launcher_id)` appears here is not
//! session-tracked: the daemon sees the Started event from the source
//! but the session manager drops it before any row is written.
//! Pre-existing sessions, aggregate stats, and the application row
//! itself survive — blocking is a go-forward silence, not a purge.

use std::collections::HashSet;

use sqlx::SqlitePool;
use time::OffsetDateTime;

use crate::error::Result;
use crate::key::GameKey;
use crate::types::LauncherType;

/// Typed access to the `blocked_applications` table.
pub struct BlockedRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> BlockedRepo<'a> {
    /// Create a new repository bound to the given pool.
    #[must_use]
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Return every blocked key. Used by the session manager at
    /// startup to hydrate its in-memory set.
    pub async fn list(&self) -> Result<HashSet<GameKey>> {
        let rows: Vec<(LauncherType, String)> =
            sqlx::query_as("SELECT launcher_type, launcher_id FROM blocked_applications")
                .fetch_all(self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(t, id)| GameKey::new(t, id))
            .collect())
    }

    /// `true` when the given key has a blocked row.
    pub async fn contains(&self, key: &GameKey) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM blocked_applications \
             WHERE launcher_type = ? AND launcher_id = ?",
        )
        .bind(key.launcher_type)
        .bind(&key.launcher_id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Add a row for `key`. Idempotent — inserting an already-blocked
    /// key is a no-op and returns `false`; newly-inserted rows
    /// return `true`.
    pub async fn insert(&self, key: &GameKey, at: OffsetDateTime) -> Result<bool> {
        let rows = sqlx::query(
            "INSERT OR IGNORE INTO blocked_applications (launcher_type, launcher_id, added_at) \
             VALUES (?, ?, ?)",
        )
        .bind(key.launcher_type)
        .bind(&key.launcher_id)
        .bind(at)
        .execute(self.pool)
        .await?
        .rows_affected();
        Ok(rows > 0)
    }

    /// Remove the row for `key`. Returns `true` when a row was
    /// deleted, `false` when the key was never blocked.
    pub async fn remove(&self, key: &GameKey) -> Result<bool> {
        let rows = sqlx::query(
            "DELETE FROM blocked_applications \
             WHERE launcher_type = ? AND launcher_id = ?",
        )
        .bind(key.launcher_type)
        .bind(&key.launcher_id)
        .execute(self.pool)
        .await?
        .rows_affected();
        Ok(rows > 0)
    }
}
