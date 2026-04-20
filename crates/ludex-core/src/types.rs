//! Enumerated values persisted to the database as TEXT.
//!
//! All types here derive [`sqlx::Type`] for SQLite TEXT storage and
//! [`strum`] helpers for `Display`, `FromStr`, and `AsRef<str>`. The on-disk
//! representation is always the snake_case form of the variant name, or an
//! explicit `#[strum(serialize = "...")]` value where a natural snake_case
//! is unavailable (for example, variant names that would start with a digit).

use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumString};

/// Origin of an application's `launcher_id`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    sqlx::Type,
    AsRefStr,
    Display,
    EnumString,
)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum LauncherType {
    /// Steam, keyed by AppID.
    Steam,
    /// Lutris, keyed by slug.
    Lutris,
    /// Heroic Games Launcher, keyed by app name.
    Heroic,
    /// Flatpak, keyed by app-id.
    Flatpak,
    /// Non-launcher-attributed application, keyed by canonical exe path.
    Native,
}

/// Graphics subsystem observed to be in use while an application was running.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    sqlx::Type,
    AsRefStr,
    Display,
    EnumString,
)]
pub enum GraphicsPlatform {
    /// DirectX DLL load observed (includes DXVK under Proton/Wine).
    #[sqlx(rename = "directx")]
    #[serde(rename = "directx")]
    #[strum(serialize = "directx")]
    DirectX,
    /// OpenGL library load observed.
    #[sqlx(rename = "opengl")]
    #[serde(rename = "opengl")]
    #[strum(serialize = "opengl")]
    OpenGL,
    /// Vulkan library load observed.
    #[sqlx(rename = "vulkan")]
    #[serde(rename = "vulkan")]
    #[strum(serialize = "vulkan")]
    Vulkan,
    /// No graphics library was identified.
    #[sqlx(rename = "unknown")]
    #[serde(rename = "unknown")]
    #[strum(serialize = "unknown")]
    Unknown,
}

/// CPU architecture reported for an application's process.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    sqlx::Type,
    AsRefStr,
    Display,
    EnumString,
)]
pub enum ProcessArchitecture {
    /// 64-bit x86.
    #[sqlx(rename = "x86_64")]
    #[serde(rename = "x86_64")]
    #[strum(serialize = "x86_64")]
    Amd64,
    /// 32-bit x86.
    #[sqlx(rename = "i686")]
    #[serde(rename = "i686")]
    #[strum(serialize = "i686")]
    I686,
    /// 64-bit ARM.
    #[sqlx(rename = "aarch64")]
    #[serde(rename = "aarch64")]
    #[strum(serialize = "aarch64")]
    Aarch64,
    /// Architecture could not be determined.
    #[sqlx(rename = "unknown")]
    #[serde(rename = "unknown")]
    #[strum(serialize = "unknown")]
    Unknown,
}

/// Cause for a session ending.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    sqlx::Type,
    AsRefStr,
    Display,
    EnumString,
)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ExitReason {
    /// The tracked process exited (observed by the `pidfd` waiter).
    Terminated,
    /// A different tracked application took the foreground.
    ForegroundChanged,
    /// The session was left open by a previous daemon run and closed at
    /// its last-known heartbeat on restart.
    Recovered,
    /// The system suspended for longer than the split threshold; the
    /// session was split at the boundary.
    SleepSplit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn assert_roundtrip<T>(variants: &[T])
    where
        T: Copy + PartialEq + std::fmt::Debug + std::fmt::Display + FromStr,
        <T as FromStr>::Err: std::fmt::Debug,
    {
        for &v in variants {
            let s = v.to_string();
            let parsed = T::from_str(&s).unwrap_or_else(|e| {
                panic!("round-trip failed for {v:?} via {s:?}: {e:?}");
            });
            assert_eq!(parsed, v, "round-trip mismatch for {v:?}");
        }
    }

    #[test]
    fn launcher_type_roundtrip() {
        assert_roundtrip(&[
            LauncherType::Steam,
            LauncherType::Lutris,
            LauncherType::Heroic,
            LauncherType::Flatpak,
            LauncherType::Native,
        ]);
    }

    #[test]
    fn graphics_platform_roundtrip() {
        assert_roundtrip(&[
            GraphicsPlatform::DirectX,
            GraphicsPlatform::OpenGL,
            GraphicsPlatform::Vulkan,
            GraphicsPlatform::Unknown,
        ]);
    }

    #[test]
    fn process_architecture_roundtrip() {
        assert_roundtrip(&[
            ProcessArchitecture::Amd64,
            ProcessArchitecture::I686,
            ProcessArchitecture::Aarch64,
            ProcessArchitecture::Unknown,
        ]);
    }

    #[test]
    fn exit_reason_roundtrip() {
        assert_roundtrip(&[
            ExitReason::Terminated,
            ExitReason::ForegroundChanged,
            ExitReason::Recovered,
            ExitReason::SleepSplit,
        ]);
    }

    #[test]
    fn amd64_serializes_as_x86_64() {
        assert_eq!(ProcessArchitecture::Amd64.to_string(), "x86_64");
        assert_eq!(
            "x86_64".parse::<ProcessArchitecture>().unwrap(),
            ProcessArchitecture::Amd64
        );
    }
}
