//! Launcher-event sources.
//!
//! Each source runs in its own tokio task and emits [`GameEvent`] values
//! onto a shared mpsc channel consumed by
//! [`SessionManager`](crate::session_manager::SessionManager). Sources
//! are independent: an error in one must not affect another.
//!
//! # Cold-start ordering
//!
//! Every source subscribes to its live event stream *before* it performs
//! any enumeration of already-running games, so events fired during the
//! enumeration are queued in the subscription and are not lost.

pub mod kwin;
pub mod steam;

pub use kwin::KWinForegroundSource;
pub use steam::SteamSource;

#[allow(
    unused_imports,
    reason = "re-exported here for symmetry with future sources"
)]
use crate::event::GameEvent;
