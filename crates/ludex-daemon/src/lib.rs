//! ludex-daemon library surface.
//!
//! The crate is compiled as both a library (for integration tests and for
//! re-use in a future `ludex` all-in-one binary) and a binary
//! (`ludex-daemon`). The binary is a thin wrapper over [`run`].

#![warn(missing_docs)]

pub mod daemon;
pub mod event;
pub mod gate;
pub mod idle;
pub mod proc;
pub mod session_manager;
pub mod sleep;
pub mod sources;

pub use daemon::{init_tracing, run};
pub use event::GameEvent;
pub use session_manager::SessionManager;
