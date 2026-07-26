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

use ludex_core::{DetectedVia, Icons, IdentityUpdate};

/// Apply `patch` on top of `acc`. No-op if `patch` is `None`.
///
/// `source` names the enricher the patch came from, and is recorded as
/// the accumulator's provenance **only when that patch supplies the
/// product name**. Deriving it here rather than letting each source
/// declare it ties the provenance to the same last-wins rule that
/// decides the name, so the two cannot disagree: a source contributing
/// only a publisher did not name the game and must not claim to have.
pub(crate) fn merge_into(
    acc: &mut IdentityUpdate,
    source: DetectedVia,
    patch: Option<IdentityUpdate>,
) {
    let Some(patch) = patch else { return };
    if patch.product_name.is_some() {
        acc.product_name = patch.product_name;
        acc.detected_via = Some(source);
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
            DetectedVia::Steam,
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
        merge_into(&mut acc, DetectedVia::Steam, None);
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
            DetectedVia::Steam,
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

    // Provenance rides on the name, not on the patch merely applying:
    // a source that contributes only a publisher did not name the game
    // and must not claim to have.
    #[test]
    fn the_source_that_supplies_the_name_is_recorded() {
        let mut acc = IdentityUpdate::default();
        merge_into(
            &mut acc,
            DetectedVia::Desktop,
            Some(IdentityUpdate {
                product_name: Some("From desktop".into()),
                ..Default::default()
            }),
        );
        assert_eq!(acc.detected_via, Some(DetectedVia::Desktop));
    }

    #[test]
    fn a_source_contributing_no_name_claims_no_provenance() {
        let mut acc = IdentityUpdate::default();
        merge_into(
            &mut acc,
            DetectedVia::Pe,
            Some(IdentityUpdate {
                publisher: Some("Some Studio".into()),
                ..Default::default()
            }),
        );
        assert_eq!(acc.detected_via, None);
    }

    // The cascade is last-wins per field, so the provenance has to move
    // with the name every time it is overwritten.
    #[test]
    fn provenance_follows_the_name_when_a_later_source_overwrites_it() {
        let mut acc = IdentityUpdate::default();
        merge_into(
            &mut acc,
            DetectedVia::Desktop,
            Some(IdentityUpdate {
                product_name: Some("Wrong name".into()),
                ..Default::default()
            }),
        );
        merge_into(
            &mut acc,
            DetectedVia::Steam,
            Some(IdentityUpdate {
                product_name: Some("Right name".into()),
                ..Default::default()
            }),
        );
        assert_eq!(acc.product_name.as_deref(), Some("Right name"));
        assert_eq!(acc.detected_via, Some(DetectedVia::Steam));
    }

    // A later source that supplies everything *but* a name leaves the
    // earlier namer's provenance standing, since its name still shows.
    #[test]
    fn provenance_survives_a_later_nameless_source() {
        let mut acc = IdentityUpdate::default();
        merge_into(
            &mut acc,
            DetectedVia::Lutris,
            Some(IdentityUpdate {
                product_name: Some("Named here".into()),
                ..Default::default()
            }),
        );
        merge_into(
            &mut acc,
            DetectedVia::Steam,
            Some(IdentityUpdate {
                publisher: Some("Later publisher".into()),
                ..Default::default()
            }),
        );
        assert_eq!(acc.detected_via, Some(DetectedVia::Lutris));
    }
}
