//! Merging logic for enrichment patches.
//!
//! An enricher returns `Option<IdentityUpdate>`; when it returns `Some`,
//! every field that is `Some` inside the patch overwrites the same field
//! in the accumulator. Fields that are `None` in the patch are skipped.
//!
//! This lets enrichers be partial — a `.desktop` file that only provides
//! a name leaves other fields alone, and a later Steam `.acf` that
//! provides only the name wins over the `.desktop` name without
//! disturbing the non-name fields set by any intermediate enricher.

use ludex_core::{Icons, IdentityUpdate};

/// Apply `patch` on top of `acc`. No-op if `patch` is `None`.
pub(crate) fn merge_into(acc: &mut IdentityUpdate, patch: Option<IdentityUpdate>) {
    let Some(patch) = patch else { return };
    if patch.product_name.is_some() {
        acc.product_name = patch.product_name;
    }
    if patch.publisher.is_some() {
        acc.publisher = patch.publisher;
    }
    if patch.version.is_some() {
        acc.version = patch.version;
    }
    if patch.executable_path.is_some() {
        acc.executable_path = patch.executable_path;
    }
    if patch.launcher_exe_path.is_some() {
        acc.launcher_exe_path = patch.launcher_exe_path;
    }
    if patch.wineprefix_path.is_some() {
        acc.wineprefix_path = patch.wineprefix_path;
    }
    if patch.installed_flatpak_ref.is_some() {
        acc.installed_flatpak_ref = patch.installed_flatpak_ref;
    }
    if patch.graphics_platform.is_some() {
        acc.graphics_platform = patch.graphics_platform;
    }
    if patch.process_architecture.is_some() {
        acc.process_architecture = patch.process_architecture;
    }
    if patch.group_id.is_some() {
        acc.group_id = patch.group_id;
    }
    merge_icons(&mut acc.icons, patch.icons);
}

fn merge_icons(acc: &mut Icons, patch: Icons) {
    if patch.icon_16.is_some() {
        acc.icon_16 = patch.icon_16;
    }
    if patch.icon_32.is_some() {
        acc.icon_32 = patch.icon_32;
    }
    if patch.icon_48.is_some() {
        acc.icon_48 = patch.icon_48;
    }
    if patch.icon_256.is_some() {
        acc.icon_256 = patch.icon_256;
    }
}

/// Returns `true` if the patch carries no changes.
pub(crate) fn is_empty(patch: &IdentityUpdate) -> bool {
    patch.product_name.is_none()
        && patch.publisher.is_none()
        && patch.version.is_none()
        && patch.executable_path.is_none()
        && patch.launcher_exe_path.is_none()
        && patch.wineprefix_path.is_none()
        && patch.installed_flatpak_ref.is_none()
        && patch.graphics_platform.is_none()
        && patch.process_architecture.is_none()
        && patch.group_id.is_none()
        && patch.icons.icon_16.is_none()
        && patch.icons.icon_32.is_none()
        && patch.icons.icon_48.is_none()
        && patch.icons.icon_256.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_default_is_empty() {
        assert!(is_empty(&IdentityUpdate::default()));
    }

    #[test]
    fn any_field_makes_non_empty() {
        let u = IdentityUpdate {
            product_name: Some("x".into()),
            ..Default::default()
        };
        assert!(!is_empty(&u));
    }

    #[test]
    fn merge_overrides_some_preserves_none() {
        let mut acc = IdentityUpdate {
            product_name: Some("old".into()),
            publisher: Some("old pub".into()),
            ..Default::default()
        };
        merge_into(
            &mut acc,
            Some(IdentityUpdate {
                product_name: Some("new".into()),
                ..Default::default()
            }),
        );
        assert_eq!(acc.product_name.as_deref(), Some("new"));
        // publisher was not in patch, must be preserved.
        assert_eq!(acc.publisher.as_deref(), Some("old pub"));
    }

    #[test]
    fn merge_none_is_noop() {
        let original = IdentityUpdate {
            product_name: Some("keep".into()),
            ..Default::default()
        };
        let mut acc = original.clone();
        merge_into(&mut acc, None);
        assert_eq!(acc.product_name, original.product_name);
    }

    #[test]
    fn icon_sizes_merge_independently() {
        let mut acc = IdentityUpdate {
            icons: Icons {
                icon_16: Some(b"small".to_vec()),
                ..Default::default()
            },
            ..Default::default()
        };
        merge_into(
            &mut acc,
            Some(IdentityUpdate {
                icons: Icons {
                    icon_32: Some(b"medium".to_vec()),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        assert_eq!(acc.icons.icon_16.as_deref(), Some(b"small".as_ref()));
        assert_eq!(acc.icons.icon_32.as_deref(), Some(b"medium".as_ref()));
    }
}
