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
/// [`SessionRepo::close_and_rollup`](crate::repo::SessionRepo::close_and_rollup)
/// on process exit, foreground change, or sleep-split. Orphaned open
/// sessions (left behind by a crash) are listed via
/// [`SessionRepo::list_all_orphans`](crate::repo::SessionRepo::list_all_orphans)
/// and closed the same way at daemon startup.
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

/// One calendar day's aggregate runtime across every tracked
/// application.
///
/// Produced by
/// [`SessionRepo::daily_playtime_since`](crate::repo::SessionRepo::daily_playtime_since).
/// Open sessions count toward the day they started on — their
/// runtime is the most recent heartbeat value, which is accurate to
/// within the heartbeat interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct DailyPlaytime {
    /// Local calendar date in `YYYY-MM-DD` form. Produced by SQLite's
    /// `DATE(…, 'localtime')` applied to the session's stored RFC 3339
    /// UTC `started_at`, using the daemon's system timezone.
    pub date: String,
    /// Sum of `full_runtime_seconds` across every session that
    /// started on this date.
    pub full_runtime_seconds: i64,
    /// Sum of `interactive_runtime_seconds` across the same sessions.
    pub interactive_runtime_seconds: i64,
    /// Number of sessions that started on this date.
    pub session_count: i64,
}

/// A session row joined against its owning application.
///
/// Produced by
/// [`SessionRepo::list_recent_with_app`](crate::repo::SessionRepo::list_recent_with_app);
/// the shape is tuned for display rather than persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct RecentSession {
    /// Session primary key.
    pub id: i64,
    /// Owning application id.
    pub application_id: i64,
    /// Product name of the owning application.
    pub product_name: String,
    /// Launcher type of the owning application.
    pub launcher_type: crate::types::LauncherType,
    /// Launcher id of the owning application.
    pub launcher_id: String,
    /// When the session was observed to begin.
    pub started_at: OffsetDateTime,
    /// When the session ended (`None` while still open).
    pub ended_at: Option<OffsetDateTime>,
    /// Accumulated full-runtime seconds.
    pub full_runtime_seconds: i64,
    /// Accumulated interactive-runtime seconds.
    pub interactive_runtime_seconds: i64,
    /// How the session ended (`None` while still open).
    pub exit_reason: Option<ExitReason>,
}
