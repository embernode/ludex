//! Events emitted by launcher sources and consumed by the session manager.

use ludex_core::GameKey;
use time::OffsetDateTime;

/// A game start or stop observed by a [`Source`](crate::sources).
///
/// Events are passed on an mpsc channel; each source clones the sender and
/// produces events independently. Two `Started` events for the same key
/// without an intervening `Stopped` are a bug in the source — the session
/// manager warns and drops the second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    /// A game is now running. If the tracker has not seen this key before,
    /// the session manager creates an [`Application`](ludex_core::Application)
    /// using `display_name` as the initial product name.
    Started {
        /// Stable identifier of the running application.
        key: GameKey,
        /// Best-effort human-readable product name. Later refined by the
        /// metadata-enrichment cascade.
        display_name: String,
        /// When the start was observed.
        at: OffsetDateTime,
    },
    /// A previously-running game has stopped.
    Stopped {
        /// Identifier previously passed in a `Started` event.
        key: GameKey,
        /// When the stop was observed.
        at: OffsetDateTime,
    },
}
