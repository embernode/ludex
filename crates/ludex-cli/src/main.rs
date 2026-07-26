//! `ludex` CLI entry point.

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod apps;
mod backup;
mod doctor;
mod merge;
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
    /// Steam/Heroic/Lutris state, DRM subsystem, and `pidfd` syscall
    /// support. Runs without contacting the daemon.
    Doctor,

    /// List recent sessions recorded by the daemon.
    Sessions {
        /// Maximum number of sessions to print.
        #[arg(long, short = 'n', default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
    },

    /// Inspect tracked applications.
    Apps {
        #[command(subcommand)]
        command: AppsCommand,
    },

    /// Manage ludex database backups.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },

    /// Fold one application row into another.
    ///
    /// Typical use: a legacy-history import lands a game as a
    /// Native-launcher row (keyed by executable path) when the
    /// live Steam source had already created the same game as a
    /// Steam-launcher row (keyed by appid). `ludex merge <src>
    /// <dst>` moves `src`'s sessions onto `dst`, sums aggregate
    /// stats, fills empty metadata slots on `dst` from `src`,
    /// then deletes `src`.
    ///
    /// Refuses to run while `ludex-daemon` is active. Look up
    /// application ids in the GUI (`/app/<id>` in the URL) or
    /// with `ludex sessions`.
    Merge {
        /// Application id that will be removed after its data is
        /// merged into the destination.
        #[arg(value_name = "SRC_ID")]
        src_id: i64,
        /// Application id that receives the source's sessions and
        /// aggregate stats. Identity (`launcher_type`, `launcher_id`,
        /// `product_name`) stays unchanged.
        #[arg(value_name = "DST_ID")]
        dst_id: i64,
    },
}

#[derive(Subcommand)]
enum AppsCommand {
    /// Print every tracked application with its id, launcher key,
    /// product name, and run count. Useful for looking up the
    /// numeric id `ludex merge` and `ludex sessions` take.
    List,
}

#[derive(Subcommand)]
enum BackupCommand {
    /// Take one snapshot now, prune older ones per the configured
    /// retention, and print the path of the new snapshot. Safe to
    /// run while `ludex-daemon` is active — SQLite `VACUUM INTO`
    /// produces a consistent copy without blocking the writer.
    Now,

    /// List every snapshot currently in the backup directory,
    /// newest first. Reports the parsed timestamp, the size on
    /// disk, and the full path.
    List,

    /// Prune older snapshots beyond the configured retention count.
    /// A `--keep` override replaces the setting for this one run
    /// only; the stored value is untouched.
    Prune {
        /// Retain this many newest snapshots; delete the rest.
        /// Clamped to a minimum of 1.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        keep: Option<u64>,
    },

    /// Restore a snapshot over the live database. Refuses to run
    /// while `ludex-daemon` is active; the daemon's open handle
    /// would make the replacement unsafe. Stops and starts of the
    /// daemon are the caller's responsibility.
    Restore {
        /// Path to the snapshot file to restore. `ludex backup list`
        /// prints paths you can copy here.
        path: std::path::PathBuf,
    },
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
        Command::Apps { command } => match command {
            AppsCommand::List => apps::list().await,
        },
        Command::Backup { command } => match command {
            BackupCommand::Now => backup::now().await,
            BackupCommand::List => backup::list().await,
            BackupCommand::Prune { keep } => backup::prune(keep).await,
            BackupCommand::Restore { path } => backup::restore(path).await,
        },
        Command::Merge { src_id, dst_id } => merge::run(src_id, dst_id).await,
    }
}
