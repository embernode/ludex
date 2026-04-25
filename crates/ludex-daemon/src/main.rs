//! ludex-daemon binary entry point.

use anyhow::Result;

/// The daemon does very little parallel work — a few D-Bus
/// subscriptions, the foreground-window gate, a sqlx pool, and
/// the periodic backup scheduler. Tokio's default
/// (`num_cpus()`-many worker threads) ends up wasteful on a
/// modern multi-core box: each worker carries a ~2 MB stack
/// and the sqlx + zbus background tasks layered on top push
/// the OS thread count further without adding throughput we'd
/// benefit from.
///
/// Two workers covers everything we do — one heartbeat tick
/// can run while a D-Bus reply is being deserialised on the
/// other. The daemon is fundamentally I/O-bound on event
/// sources, never compute-bound, so spilling parallelism into
/// more cores would be pure overhead.
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    ludex_daemon::init_tracing();
    ludex_daemon::run().await
}
