//! Wire types for the `net.ludex.Tracker1` D-Bus API.
//!
//! This crate exists to DRY the DTOs and well-known names across the
//! daemon (which serves the interface) and the Tauri GUI (which
//! consumes it through a `#[zbus::proxy]`). Both sides must agree
//! byte-for-byte on struct field order and D-Bus signatures, so
//! defining them in one place eliminates a whole category of drift.
//!
//! Deliberately free of any runtime — no tokio, no sqlx, no
//! detection code. Linking this crate into the GUI binary is almost
//! free; linking [`ludex_daemon`] would drag the entire database and
//! KWin stack along with it, which is why the duplication existed in
//! the first place.
//!
//! # API shape
//!
//! ```text
//! bus   : net.ludex.Tracker1   (session bus)
//! path  : /net/ludex/Tracker1
//! iface : net.ludex.Tracker1
//! ```
//!
//! See the daemon's `dbus` module for methods and signals; the
//! structs here describe the payloads they exchange.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

/// Well-known service name ludex claims on the user session bus.
pub const SERVICE_NAME: &str = "net.ludex.Tracker1";
/// Object path the tracker interface is exposed at.
pub const OBJECT_PATH: &str = "/net/ludex/Tracker1";
/// Interface name served at [`OBJECT_PATH`].
pub const INTERFACE: &str = "net.ludex.Tracker1";

/// Application row shaped for the GUI. Time fields are RFC 3339
/// strings; an empty string means "never" (e.g. `last_played_at`
/// for a never-played app).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ApplicationSummary {
    /// Primary-key id.
    pub id: i64,
    /// Origin of `launcher_id` (`"steam"`, `"lutris"`, `"heroic"`,
    /// `"flatpak"`, `"native"`).
    pub launcher_type: String,
    /// Identifier within the launcher.
    pub launcher_id: String,
    /// Human-readable product name.
    pub product_name: String,
    /// Publisher / developer (empty if unknown).
    pub publisher: String,
    /// Cumulative full-runtime seconds across every session.
    pub total_full_seconds: i64,
    /// Cumulative interactive-runtime seconds across every session.
    pub total_interactive_seconds: i64,
    /// Total session count.
    pub run_count: i64,
    /// RFC 3339 timestamp of the most recent session end, or empty
    /// when the app has never been played to completion.
    pub last_played_at: String,
    /// Canonical path of the game's own executable, or empty when
    /// none was recorded. Titles detected through Steam's content log
    /// have no executable on record, so an empty string is ordinary
    /// rather than exceptional.
    pub executable_path: String,
    /// RFC 3339 timestamp of when this application was first
    /// observed.
    pub first_seen_at: String,
    /// Longest single session in full-runtime seconds.
    pub longest_full_seconds: i64,
}

/// One day's worth of aggregate runtime, shaped for dashboards.
///
/// Produced by
/// [`net.ludex.Tracker1.ListDailyPlaytime`](https://net.ludex/Tracker1).
/// A day with no sessions is omitted from the reply; the GUI fills
/// gaps with zeros where the chart needs a continuous range.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DailyPlaytime {
    /// Local calendar date in `YYYY-MM-DD` form — the daemon's system
    /// timezone, which on a session-bus service is also the user's.
    /// Matches SQLite's `DATE(…, 'localtime')` applied to the
    /// session's `started_at`.
    pub date: String,
    /// Sum of full-runtime seconds across every session that started
    /// on this date.
    pub full_runtime_seconds: i64,
    /// Sum of interactive-runtime seconds across the same sessions.
    pub interactive_runtime_seconds: i64,
    /// Number of sessions that started on this date.
    pub session_count: i64,
}

/// Snapshot of the database-backup directory, shaped for the GUI.
///
/// Produced by `net.ludex.Tracker1.GetBackupStats`. Lets the
/// settings page show "you have N backups using X disk" without
/// the GUI listing files itself.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BackupStats {
    /// Absolute path to the directory ludex writes snapshots into.
    /// Always reported even when no backups exist yet, so the GUI
    /// can offer an "open folder" affordance unconditionally.
    pub directory: String,
    /// Number of `ludex-*.sqlite` files in the directory.
    pub count: u64,
    /// Cumulative byte size across every snapshot.
    pub total_bytes: u64,
    /// RFC 3339 UTC timestamp of the newest snapshot, or empty
    /// when [`count`] is zero or the newest filename has no
    /// parseable timestamp.
    ///
    /// [`count`]: BackupStats::count
    pub latest_at: String,
}

/// Session row shaped for the GUI.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SessionSummary {
    /// Primary-key id.
    pub id: i64,
    /// Owning application id.
    pub application_id: i64,
    /// Product name of the owning application (joined for
    /// convenience).
    pub product_name: String,
    /// RFC 3339 start timestamp.
    pub started_at: String,
    /// RFC 3339 end timestamp, or empty for an open session.
    pub ended_at: String,
    /// Full-runtime seconds.
    pub full_runtime_seconds: i64,
    /// Interactive-runtime seconds.
    pub interactive_runtime_seconds: i64,
    /// Reason for closure (`"terminated"`, `"foreground_changed"`,
    /// `"recovered"`, `"sleep_split"`); empty for open sessions.
    pub exit_reason: String,
    /// Primary keys of the underlying database session rows folded
    /// into this summary, newest id first. A single-element vector is
    /// a row that wasn't merged with any neighbour; more than one
    /// means the daemon collapsed consecutive same-application
    /// sessions whose end-to-start gap was shorter than the merge
    /// threshold. The fields above (started_at, runtime totals,
    /// exit_reason, …) reflect the merged span; `id` is the most
    /// recent fragment's primary key (== `fragment_ids[0]`).
    ///
    /// The GUI deletes a whole span by passing this exact id set to
    /// `delete_session`, so the rows dropped always match what was
    /// displayed — the fold runs once, here, and the delete never
    /// re-derives the span or reaches unshown older fragments
    /// (PERSIST-2).
    pub fragment_ids: Vec<i64>,
}
