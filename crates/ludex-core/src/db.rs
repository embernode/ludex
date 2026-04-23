//! Database bootstrap: open the SQLite store, enforce connection options,
//! run migrations.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::error::Result;
use crate::repo::{ApplicationRepo, BlockedRepo, SessionRepo, SettingsRepo};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// A connected ludex database.
///
/// Owns a SQLx [`SqlitePool`] with WAL journaling, synchronous=NORMAL,
/// foreign keys enabled, and a 5-second busy timeout. All migrations bundled
/// at compile time have already been applied when [`Database::open`] returns.
#[derive(Debug, Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Open a database at a filesystem path, creating the file if missing,
    /// applying all pending migrations, and enforcing the standard pragma
    /// set.
    ///
    /// The path is passed to SQLite verbatim — characters that would
    /// confuse a URL parser (spaces, `?`, `#`, `%`, unicode) survive
    /// unscathed. Prefer this over [`Database::open_url`] for any caller
    /// that already has a [`Path`].
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let options = SqliteConnectOptions::new().filename(path.as_ref());
        Self::open_with(options).await
    }

    /// Open a purely in-memory database. Exists for tests and the
    /// `ludex doctor` / `--dry-run` pathways; real users always go through
    /// [`Database::open`].
    pub async fn open_memory() -> Result<Self> {
        Self::open_url("sqlite::memory:").await
    }

    /// Open by raw SQLx connection URL. Only for callers that genuinely
    /// have a URL (for example `sqlite::memory:` in tests); filesystem
    /// paths should go through [`Database::open`] instead, which
    /// avoids URL-encoding hazards.
    pub async fn open_url(url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?;
        Self::open_with(options).await
    }

    async fn open_with(options: SqliteConnectOptions) -> Result<Self> {
        let options = options
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        MIGRATOR.run(&pool).await?;

        Ok(Self { pool })
    }

    /// Access the underlying pool for callers that need it
    /// (transactions, cross-repository queries).
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Construct an [`ApplicationRepo`] bound to this database's pool.
    #[must_use]
    pub fn applications(&self) -> ApplicationRepo<'_> {
        ApplicationRepo::new(&self.pool)
    }

    /// Construct a [`SessionRepo`] bound to this database's pool.
    #[must_use]
    pub fn sessions(&self) -> SessionRepo<'_> {
        SessionRepo::new(&self.pool)
    }

    /// Construct a [`SettingsRepo`] bound to this database's pool.
    #[must_use]
    pub fn settings(&self) -> SettingsRepo<'_> {
        SettingsRepo::new(&self.pool)
    }

    /// Construct a [`BlockedRepo`] bound to this database's pool.
    #[must_use]
    pub fn blocked(&self) -> BlockedRepo<'_> {
        BlockedRepo::new(&self.pool)
    }

    /// Close the pool. Callers can simply drop [`Database`] instead; this
    /// is a deterministic variant for shutdown paths.
    pub async fn close(self) {
        self.pool.close().await;
    }
}
