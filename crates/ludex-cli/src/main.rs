//! `ludex` CLI entry point.

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod doctor;

#[derive(Parser)]
#[command(
    name = "ludex",
    about = "ludex command-line interface",
    version,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a capability table for the current environment.
    ///
    /// Verifies presence and reachability of the components ludex needs:
    /// session type (Wayland/X11), desktop, KWin D-Bus, logind D-Bus,
    /// Steam/Heroic/Lutris state, DRM subsystem, `input` group membership,
    /// and `pidfd` syscall support. Runs without contacting the daemon.
    Doctor,
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("LUDEX_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => doctor::run().await,
    }
}
