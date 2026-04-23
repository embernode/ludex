//! `ludex` CLI entry point.

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod backup;
mod doctor;
mod sessions;

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

    /// List recent sessions recorded by the daemon.
    Sessions {
        /// Maximum number of sessions to print.
        #[arg(long, short = 'n', default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
    },

    /// Manage ludex database backups.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
}

#[derive(Subcommand)]
enum BackupCommand {
    /// Take one snapshot now, prune older ones per the configured
    /// retention, and print the path of the new snapshot. Safe to
    /// run while `ludex-daemon` is active — SQLite `VACUUM INTO`
    /// produces a consistent copy without blocking the writer.
    Now,
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
        Command::Sessions { limit } => sessions::run(limit).await,
        Command::Backup { command } => match command {
            BackupCommand::Now => backup::now().await,
        },
    }
}
