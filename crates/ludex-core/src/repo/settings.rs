//! Key/value settings persistence.
//!
//! Daemon-wide configuration the user can tweak from the GUI lives
//! here. Values are stored as TEXT and the typed accessors below
//! parse them — adding a new setting is a method on this repo, never
//! a schema migration.

use sqlx::SqlitePool;

use crate::error::{Error, Result};

/// Key for the per-process GPU memory threshold the gate uses to
/// accept a non-fullscreen window as a game. Stored as bytes.
pub const GPU_MEMORY_THRESHOLD_BYTES: &str = "gpu_memory_threshold_bytes";

/// Default for [`GPU_MEMORY_THRESHOLD_BYTES`]: 50 MiB. Matches the
/// value hard-coded into `GateConfig::default` before M6.6 made it
/// user-configurable.
pub const DEFAULT_GPU_MEMORY_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024;

/// Typed access to the `settings` table.
pub struct SettingsRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SettingsRepo<'a> {
    /// Create a new repository bound to the given pool.
    #[must_use]
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Return the raw string value stored under `key`, if any.
    pub async fn get_raw(&self, key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(self.pool)
            .await?;
        Ok(row.map(|(v,)| v))
    }

    /// Store `value` under `key`, inserting a new row or replacing an
    /// existing one. Empty-string values are rejected so that an
    /// absent setting and a present-but-blank one are never confused.
    pub async fn set_raw(&self, key: &str, value: &str) -> Result<()> {
        if value.is_empty() {
            return Err(Error::Invariant("settings value must not be empty"));
        }
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Remove the row for `key`. Returns `true` if a row was deleted.
    pub async fn remove(&self, key: &str) -> Result<bool> {
        let rows = sqlx::query("DELETE FROM settings WHERE key = ?")
            .bind(key)
            .execute(self.pool)
            .await?
            .rows_affected();
        Ok(rows > 0)
    }

    /// Get a `u64` setting, returning `fallback` when the row is
    /// absent. An unparseable value is surfaced as `Error::Invariant`
    /// rather than silently masked — the GUI or CLI that wrote it
    /// should never produce one.
    pub async fn get_u64(&self, key: &str, fallback: u64) -> Result<u64> {
        match self.get_raw(key).await? {
            None => Ok(fallback),
            Some(s) => s
                .parse::<u64>()
                .map_err(|_| Error::Invariant("settings value is not a valid u64")),
        }
    }

    /// Store a `u64` setting.
    pub async fn set_u64(&self, key: &str, value: u64) -> Result<()> {
        self.set_raw(key, &value.to_string()).await
    }
}
