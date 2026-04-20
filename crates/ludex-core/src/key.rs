//! Stable identifier for applications.

use serde::{Deserialize, Serialize};

use crate::types::LauncherType;

/// Composite identity of a tracked application.
///
/// `(launcher_type, launcher_id)` is the natural key: launcher IDs survive
/// install-path changes, updates, and library moves, whereas filesystem
/// paths do not. For applications detected outside any launcher, the
/// `Native` variant uses the canonical executable path as its id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GameKey {
    /// Origin of the launcher identifier.
    pub launcher_type: LauncherType,
    /// Identifier as understood by the launcher.
    pub launcher_id: String,
}

impl GameKey {
    /// Construct a key from its parts.
    #[must_use]
    pub fn new(launcher_type: LauncherType, launcher_id: impl Into<String>) -> Self {
        Self {
            launcher_type,
            launcher_id: launcher_id.into(),
        }
    }

    /// Construct a Steam AppID key.
    #[must_use]
    pub fn steam(appid: impl Into<String>) -> Self {
        Self::new(LauncherType::Steam, appid)
    }

    /// Construct a Lutris slug key.
    #[must_use]
    pub fn lutris(slug: impl Into<String>) -> Self {
        Self::new(LauncherType::Lutris, slug)
    }

    /// Construct a Heroic app-name key.
    #[must_use]
    pub fn heroic(app_name: impl Into<String>) -> Self {
        Self::new(LauncherType::Heroic, app_name)
    }

    /// Construct a Flatpak app-id key.
    #[must_use]
    pub fn flatpak(app_id: impl Into<String>) -> Self {
        Self::new(LauncherType::Flatpak, app_id)
    }

    /// Construct a native (non-launcher) key from a canonical exe path.
    #[must_use]
    pub fn native(exe_path: impl Into<String>) -> Self {
        Self::new(LauncherType::Native, exe_path)
    }
}

impl std::fmt::Display for GameKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.launcher_type, self.launcher_id)
    }
}
