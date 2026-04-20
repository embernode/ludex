//! Session records and related value types.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;

use crate::types::ExitReason;

/// One continuous period during which an application was being played.
///
/// A session is created by [`SessionRepo::begin`](crate::repo::SessionRepo::begin)
/// when the detector accepts an application. It is updated at heartbeat
/// intervals (default 60 s), and closed by
/// [`SessionRepo::end`](crate::repo::SessionRepo::end) on process exit,
/// foreground change, or sleep-split. Orphaned open sessions (left behind
/// by a crash) are closed by
/// [`SessionRepo::recover_orphans`](crate::repo::SessionRepo::recover_orphans)
/// at daemon startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Session {
    /// Primary key.
    pub id: i64,
    /// Foreign key into `applications`.
    pub application_id: i64,

    /// When the session was observed to begin.
    pub started_at: OffsetDateTime,
    /// When the session ended. `None` means the session is still open
    /// (either currently playing, or orphaned awaiting recovery).
    pub ended_at: Option<OffsetDateTime>,
    /// Last recorded heartbeat from the daemon.
    pub heartbeat_at: OffsetDateTime,

    /// Wall-clock seconds from session start to the most recent heartbeat
    /// (or to `ended_at` once closed).
    pub full_runtime_seconds: i64,
    /// `full_runtime_seconds` minus idle intervals reported by
    /// `logind.IdleHint`.
    pub interactive_runtime_seconds: i64,

    /// How the session ended. `None` only while the session is open.
    pub exit_reason: Option<ExitReason>,
}

/// Heartbeat update applied by the session manager while a session is open.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeSnapshot {
    /// Cumulative full-runtime seconds since session start.
    pub full_runtime_seconds: i64,
    /// Cumulative interactive-runtime seconds since session start.
    pub interactive_runtime_seconds: i64,
    /// Timestamp at which this snapshot was taken.
    pub at: OffsetDateTime,
}
