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

/// Key for the interval (hours) between periodic database backups
/// the daemon takes while running. Independent of shutdown backups,
/// which always fire on a clean stop.
pub const BACKUP_INTERVAL_HOURS: &str = "backup_interval_hours";

/// Default for [`BACKUP_INTERVAL_HOURS`]: once a day.
pub const DEFAULT_BACKUP_INTERVAL_HOURS: u64 = 24;

/// Key for the number of database backups to retain. Older files
/// are pruned after each successful snapshot.
pub const BACKUP_RETENTION_COUNT: &str = "backup_retention_count";

/// Default for [`BACKUP_RETENTION_COUNT`]: two weeks of dailies.
pub const DEFAULT_BACKUP_RETENTION_COUNT: u64 = 14;

/// Key for the grace window (seconds) between the tracked game
/// losing foreground and the session actually closing. Exists so
/// alt-tabbing to a browser and back doesn't split one session into
/// many and inflate the per-application run count.
pub const ALT_TAB_GRACE_SECONDS: &str = "alt_tab_grace_seconds";

/// Default for [`ALT_TAB_GRACE_SECONDS`]: fifteen seconds. Long
/// enough to cover a quick look at a chat window; short enough that
/// a real session end is recorded close to the user's perception of
/// "I stopped playing".
pub const DEFAULT_ALT_TAB_GRACE_SECONDS: u64 = 15;

/// Key for whether losing foreground focus should pause the session
/// at all. When `true`, the foreground source enters the grace
/// window described above and eventually closes the session. When
/// `false`, background focus is ignored entirely — sessions only
/// end on process exit. Mirrors the "do not pause when out of
/// focus" toggle users asked for to match prior tools.
pub const PAUSE_WHEN_BACKGROUNDED: &str = "pause_when_backgrounded";

/// Default for [`PAUSE_WHEN_BACKGROUNDED`]: `true`, matching the
/// pre-existing behaviour. Opt in to the "always count as
/// playing" mode rather than changing what existing users see.
pub const DEFAULT_PAUSE_WHEN_BACKGROUNDED: bool = true;

/// Key for the per-idle-interval grace (seconds). The first
/// `idle_grace_seconds` of every input-idle interval are credited
/// to `interactive_runtime_seconds` rather than subtracted as AFK
/// time. The intent is to forgive non-skippable cutscenes,
/// dialogue trees, long animations, and similar engagement-
/// without-input events that today read as "user stepped away".
/// Genuine AFK longer than the grace still bills correctly: only
/// the first `grace` seconds of each natural interval are
/// forgiven, the tail is subtracted as before.
pub const IDLE_GRACE_SECONDS: &str = "idle_grace_seconds";

/// Default for [`IDLE_GRACE_SECONDS`]: three minutes. Because the
/// grace is forgiven per interval rather than per session, it is
/// uncapped across a session — a five-minute default forgave up to
/// five minutes for *every* input-free stretch, which in practice
/// meant only one continuous AFK longer than five minutes ever
/// billed at all. Three minutes still covers a typical cutscene
/// while leaving genuine AFK visible. Players with very long
/// cutscenes (Metal Gear, Final Fantasy, Naughty Dog titles) can
/// crank this higher from Settings.
pub const DEFAULT_IDLE_GRACE_SECONDS: u64 = 3 * 60;

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

    /// Get a `bool` setting, returning `fallback` when the row is
    /// absent. Accepts both numeric (`"0"` / `"1"`) and textual
    /// (`"true"` / `"false"`) representations so a value written
    /// by hand into the DB is still honoured.
    pub async fn get_bool(&self, key: &str, fallback: bool) -> Result<bool> {
        match self.get_raw(key).await? {
            None => Ok(fallback),
            Some(s) => match s.as_str() {
                "1" | "true" => Ok(true),
                "0" | "false" => Ok(false),
                _ => Err(Error::Invariant("settings value is not a valid bool")),
            },
        }
    }

    /// Store a `bool` setting. Written as `"1"` / `"0"` for a
    /// compact, locale-agnostic on-disk form.
    pub async fn set_bool(&self, key: &str, value: bool) -> Result<()> {
        self.set_raw(key, if value { "1" } else { "0" }).await
    }
}
