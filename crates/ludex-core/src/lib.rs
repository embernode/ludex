//! Shared types, schema definitions, and storage layer for ludex.
//!
//! This crate owns the SQLite schema, the domain types
//! ([`Application`], [`Session`], [`GameKey`], enumerations), and the
//! typed repositories ([`repo::ApplicationRepo`], [`repo::SessionRepo`])
//! that wrap SQL behind domain-shaped methods. The daemon and CLI depend
//! on `ludex-core`; no SQL should appear anywhere else.
//!
//! # Opening a database
//!
//! ```no_run
//! # use ludex_core::Database;
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::open("/tmp/ludex.sqlite").await?;
//! let apps = db.applications().list().await?;
//! # Ok(()) }
//! ```

#![warn(missing_docs)]

mod application;
mod db;
pub mod error;
mod key;
mod paths;
pub mod repo;
mod session;
mod types;
pub mod vdf;

pub use application::{Application, Icons, IdentityUpdate, NewApplication, PlaybackDelta};
pub use db::Database;
pub use error::{Error, Result};
pub use key::GameKey;
pub use paths::default_database_path;
pub use session::{DailyPlaytime, RecentSession, RuntimeSnapshot, Session};
pub use types::{ExitReason, GraphicsPlatform, LauncherType, ProcessArchitecture};
