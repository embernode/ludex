//! ludex-daemon binary entry point.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    ludex_daemon::init_tracing();
    ludex_daemon::run().await
}
