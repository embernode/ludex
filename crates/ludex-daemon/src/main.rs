//! ludex-daemon entry point.
//!
//! Milestone M0: scaffold only. Real detection work lands in M2.

use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_env("LUDEX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    tracing::info!("ludex-daemon starting (M0 placeholder — no detection yet)");
    Ok(())
}
