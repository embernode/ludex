//! Metadata enrichment cascade for ludex.
//!
//! Given an [`Application`] row that was just inserted from a launcher
//! `Started` event, fill in its identity fields (product name, publisher,
//! version, graphics platform, etc.) by consulting every available
//! source of truth on the system — `.desktop` files, Steam
//! `appmanifest_*.acf`, GOG `goggame-*.info`, PE `FileVersionInfo`, and
//! others.
//!
//! Design:
//!
//! - Each source is an `async fn` returning `Option<IdentityUpdate>`.
//!   `None` means "this source does not apply to this application"
//!   (e.g. PE FileVersionInfo on a native Linux exe). Errors inside a
//!   source are logged and treated as `None`; one bad source never
//!   poisons the cascade.
//! - [`run_cascade`] invokes sources in ascending-priority order. Each
//!   source's non-empty fields overwrite the accumulated patch, so the
//!   last source wins. The order is set by [`build_patch`].
//! - The accumulated patch is applied in a single
//!   [`ApplicationRepo::update_identity`](ludex_core::repo::ApplicationRepo::update_identity)
//!   call, guaranteeing atomicity.

#![warn(missing_docs)]

use ludex_core::{Application, Database, IdentityUpdate};
use tracing::{debug, instrument, warn};

pub mod context;
mod merge;
pub mod sources;

pub use context::EnrichmentContext;

/// Run the full enrichment cascade against the application with `app_id`
/// and persist the merged patch. A no-op if no source contributes any
/// field.
#[instrument(skip(db, ctx), fields(app_id))]
pub async fn run_cascade(
    db: &Database,
    ctx: &EnrichmentContext,
    app_id: i64,
) -> Result<(), ludex_core::Error> {
    let Some(app) = db.applications().find_by_id(app_id).await? else {
        warn!("application not found; enrichment skipped");
        return Ok(());
    };

    let patch = build_patch(&app, ctx).await;
    if merge::is_empty(&patch) {
        debug!("no enrichment sources produced data");
        return Ok(());
    }

    db.applications().update_identity(app_id, patch).await?;
    Ok(())
}

/// Apply every enricher in priority order and return the merged patch.
///
/// Order (lowest priority first, so higher-priority sources overwrite):
///
/// 1. `.desktop` entry
/// 2. PE `FileVersionInfo` (Proton/Wine games only; stubbed in this tranche)
/// 3. GOG `goggame-*.info` (stubbed in this tranche)
/// 4. Heroic store JSON (stubbed in this tranche)
/// 5. Lutris `pga.db` (stubbed in this tranche)
/// 6. Steam `appmanifest_*.acf`
pub async fn build_patch(app: &Application, ctx: &EnrichmentContext) -> IdentityUpdate {
    let mut acc = IdentityUpdate::default();
    merge::merge_into(&mut acc, sources::desktop::enrich(app, ctx).await);
    merge::merge_into(&mut acc, sources::steam::enrich(app, ctx).await);
    acc
}
