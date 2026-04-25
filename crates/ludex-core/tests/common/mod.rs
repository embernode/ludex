//! Helpers shared between the per-repo integration test files.
//!
//! Cargo compiles each `tests/*.rs` as its own test binary, so any
//! helper that's needed in more than one place lands here in
//! `tests/common/mod.rs` (the `mod.rs` form keeps Cargo from
//! treating `common` itself as a separate test crate). Each test
//! file `mod common;`s in what it uses; unused parts of this
//! module would otherwise trigger `dead_code` warnings, so the
//! `allow` lives at the module level.

#![allow(dead_code)]

use ludex_core::{GraphicsPlatform, Icons, LauncherType, NewApplication, ProcessArchitecture};
use time::OffsetDateTime;

/// Reasonable default `NewApplication` for tests that need an
/// application row to attach sessions / blocked entries / merges
/// to. Caller adjusts launcher_type / launcher_id / product_name
/// as needed for the scenario.
pub(crate) fn sample_new_app() -> NewApplication {
    NewApplication {
        launcher_type: LauncherType::Steam,
        launcher_id: "440".into(),
        product_name: "Team Fortress 2".into(),
        publisher: Some("Valve".into()),
        version: None,
        executable_path: Some(
            "/home/x/.local/share/Steam/steamapps/common/Team Fortress 2/hl2_linux".into(),
        ),
        launcher_exe_path: None,
        wineprefix_path: None,
        installed_flatpak_ref: None,
        graphics_platform: GraphicsPlatform::OpenGL,
        process_architecture: ProcessArchitecture::Amd64,
        group_id: None,
        icons: Icons::default(),
        first_seen_at: OffsetDateTime::now_utc(),
    }
}
