//! Typed repositories that wrap SQL behind domain-shaped methods.
//!
//! No SQL escapes these modules; the rest of the workspace calls only the
//! methods exposed here. This gives us a single place to enforce invariants
//! (idle seconds never exceed full seconds, orphan recovery is atomic,
//! etc.) and a single place to change the wire format.

mod application;
mod blocked;
mod session;
mod settings;

pub use application::ApplicationRepo;
pub use blocked::BlockedRepo;
pub use session::SessionRepo;
pub use settings::{
    SettingsRepo, ALT_TAB_GRACE_SECONDS, BACKUP_INTERVAL_HOURS, BACKUP_RETENTION_COUNT,
    DEFAULT_ALT_TAB_GRACE_SECONDS, DEFAULT_BACKUP_INTERVAL_HOURS, DEFAULT_BACKUP_RETENTION_COUNT,
    DEFAULT_GPU_MEMORY_THRESHOLD_BYTES, DEFAULT_PAUSE_WHEN_BACKGROUNDED,
    GPU_MEMORY_THRESHOLD_BYTES, PAUSE_WHEN_BACKGROUNDED,
};
