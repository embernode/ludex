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
pub use settings::{SettingsRepo, DEFAULT_GPU_MEMORY_THRESHOLD_BYTES, GPU_MEMORY_THRESHOLD_BYTES};
