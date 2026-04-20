//! Error types for the core layer.

use thiserror::Error;

/// Convenience alias for results from this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors produced by the core storage layer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// An underlying SQL operation failed.
    #[error("database error: {0}")]
    Sql(#[from] sqlx::Error),

    /// A schema migration failed.
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    /// A text value read from the database does not match any known variant
    /// of the expected enumeration. This normally indicates a schema-code
    /// drift: the migration CHECK constraint lists a value the Rust enum
    /// does not know about (or vice versa).
    #[error("unknown value {value:?} for field {field}")]
    UnknownVariant {
        /// The enum whose variants were considered.
        field: &'static str,
        /// The offending raw value.
        value: String,
    },

    /// A database row violated an invariant the domain layer expects to
    /// hold. This only surfaces when the CHECK constraints and the Rust
    /// types disagree, which should be impossible in practice.
    #[error("invariant violated: {0}")]
    Invariant(&'static str),
}
