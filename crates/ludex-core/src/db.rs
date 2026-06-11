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
        Self::open_with(options, Self::default_pool_options()).await
    }

    /// Open a purely in-memory database. Only tests use this; real
    /// callers always go through [`Database::open`].
    pub async fn open_memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?;
        // sqlx maps `sqlite::memory:` onto a named shared-cache
        // in-memory database that lives exactly as long as at least
        // one pool connection stays open. Pin the pool to a single
        // never-recycled connection so idle reaping can never drop
        // the only copy of the data, and so shared-cache table locks
        // can't surface under concurrent acquires.
        let pool_options = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None);
        Self::open_with(options, pool_options).await
    }

    /// Open by raw SQLx connection URL. Only for callers that genuinely
    /// have a URL; filesystem paths should go through [`Database::open`]
    /// instead, which avoids URL-encoding hazards, and in-memory callers
    /// through [`Database::open_memory`], which pins the pool so the
    /// data can't be dropped by connection recycling.
    pub async fn open_url(url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?;
        Self::open_with(options, Self::default_pool_options()).await
    }

    fn default_pool_options() -> SqlitePoolOptions {
        SqlitePoolOptions::new().max_connections(4)
    }

    async fn open_with(
        options: SqliteConnectOptions,
        pool_options: SqlitePoolOptions,
    ) -> Result<Self> {
        let options = options
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));

        let pool = pool_options.connect_with(options).await?;

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
