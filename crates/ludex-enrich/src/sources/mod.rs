//! Individual enrichment sources.
//!
//! Each module exposes a single `async fn enrich(app, ctx) ->
//! Option<IdentityUpdate>`. The cascade in [`crate::build_patch`] invokes
//! them in priority order.

pub mod desktop;
pub mod steam;
