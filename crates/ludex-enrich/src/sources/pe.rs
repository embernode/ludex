//! PE `FileVersionInfo` enricher.
//!
//! For Proton/Wine games the on-disk executable is a Windows PE that
//! still carries its `VS_VERSIONINFO` resource. `pelite` lets us parse
//! that resource natively on Linux without Wine. We extract
//! `ProductName`, `CompanyName`, and the best available version string
//! (`ProductVersion` is preferred; `FileVersion` is the fallback).
//!
//! Only applications whose `executable_path` ends in `.exe` are
//! considered. Anything else (including native ELF binaries with a
//! spurious `.exe` suffix) is silently skipped via the PE magic check
//! inside `pelite`.

use std::path::{Path, PathBuf};

use ludex_core::{Application, IdentityUpdate};
use pelite::resources::version_info::VersionInfo;
use tracing::debug;

use crate::context::EnrichmentContext;

const MAX_PE_READ_BYTES: u64 = 64 * 1024 * 1024;

/// Enrich an application from its Windows PE `VS_VERSIONINFO` resource.
pub async fn enrich(app: &Application, _ctx: &EnrichmentContext) -> Option<IdentityUpdate> {
    let exe = app.executable_path.as_ref()?;
    let path = Path::new(exe);
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    let path_buf = path.to_path_buf();

    // Parsing is CPU-bound and pelite is a synchronous library. Hand it
    // a blocking worker so we don't stall the tokio scheduler on the
    // occasional large PE.
    tokio::task::spawn_blocking(move || read_version_info(&path_buf))
        .await
        .ok()
        .flatten()
}

fn read_version_info(path: &PathBuf) -> Option<IdentityUpdate> {
    // Guard against absurdly large files: PE executables over 64 MiB
    // are uncommon and we'd rather skip than OOM on a hostile file.
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_PE_READ_BYTES {
        debug!(path = %path.display(), size = metadata.len(), "PE too large; skipping");
        return None;
    }

    let bytes = std::fs::read(path).ok()?;
    let update = extract_64(&bytes).or_else(|| extract_32(&bytes))?;
    debug!(path = %path.display(), "PE enricher matched");
    Some(update)
}

fn extract_64(bytes: &[u8]) -> Option<IdentityUpdate> {
    use pelite::pe64::{Pe, PeFile};
    let pe = PeFile::from_bytes(bytes).ok()?;
    let version_info = pe.resources().ok()?.version_info().ok()?;
    extract_from_version_info(&version_info)
}

fn extract_32(bytes: &[u8]) -> Option<IdentityUpdate> {
    use pelite::pe32::{Pe, PeFile};
    let pe = PeFile::from_bytes(bytes).ok()?;
    let version_info = pe.resources().ok()?.version_info().ok()?;
    extract_from_version_info(&version_info)
}

fn extract_from_version_info(vi: &VersionInfo<'_>) -> Option<IdentityUpdate> {
    // Prefer the English-US localisation if present; otherwise take
    // whichever translation is listed first.
    const LANG_EN_US: u16 = 0x0409;
    let lang = vi
        .translation()
        .iter()
        .copied()
        .find(|t| t.lang_id == LANG_EN_US)
        .or_else(|| vi.translation().iter().copied().next())?;

    let mut product_name: Option<String> = None;
    let mut file_description: Option<String> = None;
    let mut publisher: Option<String> = None;
    let mut product_version: Option<String> = None;
    let mut file_version: Option<String> = None;

    vi.strings(lang, |k, v| match k {
        "ProductName" => product_name = non_empty(v),
        "FileDescription" => file_description = non_empty(v),
        "CompanyName" => publisher = non_empty(v),
        "ProductVersion" => product_version = non_empty(v),
        "FileVersion" => file_version = non_empty(v),
        _ => {}
    });

    let product_name = product_name.or(file_description);
    let version = product_version.or(file_version);
    if product_name.is_none() && publisher.is_none() && version.is_none() {
        return None;
    }
    Some(IdentityUpdate {
        product_name,
        publisher,
        version,
        ..Default::default()
    })
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::non_empty;

    #[test]
    fn non_empty_strips_whitespace() {
        assert_eq!(non_empty("hello"), Some("hello".into()));
        assert_eq!(non_empty("  padded  "), Some("padded".into()));
        assert_eq!(non_empty(""), None);
        assert_eq!(non_empty("   "), None);
    }
}
