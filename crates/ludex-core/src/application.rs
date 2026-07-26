//! Application records and related value types.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;

use crate::types::{DetectedVia, GraphicsPlatform, LauncherType, ProcessArchitecture};

/// An application known to ludex.
///
/// Populated by the detector / enrichment cascade and updated by the session
/// manager on session close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Application {
    /// Surrogate primary key. Stable across renames and re-enrichment.
    pub id: i64,
    /// Origin of [`Application::launcher_id`].
    pub launcher_type: LauncherType,
    /// Identifier as understood by the launcher.
    pub launcher_id: String,

    /// Human-readable product name.
    pub product_name: String,
    /// Publisher or developer (from enrichment).
    pub publisher: Option<String>,
    /// Version string (from enrichment, typically PE `FileVersionInfo`
    /// for Proton/Wine games).
    pub version: Option<String>,

    /// Canonical path of the process's main executable (the game itself,
    /// not the launcher wrapper).
    pub executable_path: Option<String>,
    /// Launcher executable chain, if different from `executable_path`.
    pub launcher_exe_path: Option<String>,
    /// Wine/Proton prefix, if the application ran under one.
    pub wineprefix_path: Option<String>,
    /// Flatpak ref, if the application was observed inside a Flatpak
    /// sandbox.
    pub installed_flatpak_ref: Option<String>,

    /// Graphics subsystem last observed in the process.
    pub graphics_platform: GraphicsPlatform,
    /// Architecture of the process.
    pub process_architecture: ProcessArchitecture,

    /// User-facing classification.
    pub group_id: Option<i64>,

    /// Which enrichment source supplied [`Application::product_name`],
    /// or `None` when no source did or the row predates the field.
    ///
    /// A plain `String` rather than [`DetectedVia`] deliberately: this
    /// column carries no schema `CHECK`, so a value written by a newer
    /// build (or restored from a backup) must not fail to decode. As an
    /// enum it would abort the entire row read, taking the whole
    /// library listing with it over a caption. Writers use the enum;
    /// readers accept whatever is there and let the interface render an
    /// unfamiliar value as itself.
    pub detected_via: Option<String>,

    /// 16x16 icon.
    #[sqlx(default)]
    pub icon_16: Option<Vec<u8>>,
    /// 32x32 icon.
    #[sqlx(default)]
    pub icon_32: Option<Vec<u8>>,
    /// 48x48 icon.
    #[sqlx(default)]
    pub icon_48: Option<Vec<u8>>,
    /// 256x256 icon.
    #[sqlx(default)]
    pub icon_256: Option<Vec<u8>>,

    /// When this application was first observed.
    pub first_seen_at: OffsetDateTime,
    /// When the most recent session ended (or `None` if never played).
    pub last_played_at: Option<OffsetDateTime>,

    /// Total session count.
    pub stat_run_count: i64,
    /// Sum of full-runtime seconds across all sessions.
    pub stat_total_full: i64,
    /// Sum of interactive-runtime seconds across all sessions.
    pub stat_total_interactive: i64,
    /// Longest single session in full-runtime seconds.
    pub stat_longest_full: i64,
}

/// Four standard icon sizes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Icons {
    /// 16x16.
    pub icon_16: Option<Vec<u8>>,
    /// 32x32.
    pub icon_32: Option<Vec<u8>>,
    /// 48x48.
    pub icon_48: Option<Vec<u8>>,
    /// 256x256.
    pub icon_256: Option<Vec<u8>>,
}

/// Data required to insert a new [`Application`].
///
/// All fields except `launcher_type`, `launcher_id`, and `product_name`
/// default to sensible empty / unknown values. The enrichment cascade
/// fills in the rest via [`IdentityUpdate`].
#[derive(Debug, Clone)]
pub struct NewApplication {
    /// See [`Application::launcher_type`].
    pub launcher_type: LauncherType,
    /// See [`Application::launcher_id`].
    pub launcher_id: String,
    /// See [`Application::product_name`].
    pub product_name: String,
    /// See [`Application::publisher`].
    pub publisher: Option<String>,
    /// See [`Application::version`].
    pub version: Option<String>,
    /// See [`Application::executable_path`].
    pub executable_path: Option<String>,
    /// See [`Application::launcher_exe_path`].
    pub launcher_exe_path: Option<String>,
    /// See [`Application::wineprefix_path`].
    pub wineprefix_path: Option<String>,
    /// See [`Application::installed_flatpak_ref`].
    pub installed_flatpak_ref: Option<String>,
    /// See [`Application::graphics_platform`].
    pub graphics_platform: GraphicsPlatform,
    /// See [`Application::process_architecture`].
    pub process_architecture: ProcessArchitecture,
    /// See [`Application::group_id`].
    pub group_id: Option<i64>,
    /// Embedded icon bytes in up to four sizes.
    pub icons: Icons,
    /// When the application was first observed. Typically `OffsetDateTime::now_utc()`.
    pub first_seen_at: OffsetDateTime,
}

/// Patch applied by the enrichment cascade after a successful source
/// lookup (`.desktop` file, Steam `.acf`, PE version info, etc.).
///
/// `Some(v)` replaces the existing value, `None` leaves it unchanged.
/// Enrichment is append-only in v1: there is no "clear to NULL"
/// capability. Administrative clearing is a post-M6 feature.
#[derive(Debug, Clone, Default)]
pub struct IdentityUpdate {
    /// Replaces `product_name` if present.
    pub product_name: Option<String>,
    /// Replaces `publisher`.
    pub publisher: Option<String>,
    /// Replaces `version`.
    pub version: Option<String>,
    /// Replaces `executable_path`.
    pub executable_path: Option<String>,
    /// Replaces `launcher_exe_path`.
    pub launcher_exe_path: Option<String>,
    /// Replaces `wineprefix_path`.
    pub wineprefix_path: Option<String>,
    /// Replaces `installed_flatpak_ref`.
    pub installed_flatpak_ref: Option<String>,
    /// Replaces `graphics_platform`.
    pub graphics_platform: Option<GraphicsPlatform>,
    /// Replaces `process_architecture`.
    pub process_architecture: Option<ProcessArchitecture>,
    /// Replaces `group_id`.
    pub group_id: Option<i64>,
    /// Which source supplied `product_name`.
    ///
    /// Set by the enrichment cascade's merge step rather than by a
    /// source itself, so it always names whichever source's name
    /// actually survived the last-wins merge.
    pub detected_via: Option<DetectedVia>,
    /// Replaces any subset of the icon fields whose value is `Some`.
    pub icons: Icons,
}

/// Delta applied to an application's aggregate statistics on session close.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackDelta {
    /// Full-runtime seconds to add to the total.
    pub full_runtime_seconds: i64,
    /// Interactive-runtime seconds to add to the total.
    pub interactive_runtime_seconds: i64,
    /// New longest full-runtime, if this session exceeded the previous
    /// record. `None` leaves `stat_longest_full` unchanged.
    pub longest_full_candidate: Option<i64>,
    /// Moment the session ended, used to update `last_played_at`.
    pub last_played_at: OffsetDateTime,
}
