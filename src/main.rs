//! token-counter entry point.
//!
//! Supports two interaction modes sharing the same core pipeline:
//! - Interactive TUI mode (default when no subcommand is given)
//! - One-shot CLI report mode (`daily`, `hourly`, `summary`)

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};

/// Supported data sources for the current scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AgentFilter {
    All,
    Opencode,
    Dsh,
    Agy,
}

/// Top-level CLI arguments.
#[derive(Debug, Parser)]
#[command(
    name = "token-counter",
    version,
    about = "Token usage, cost, and speed analytics for AI coding agents"
)]
struct Cli {
    /// Subcommand for one-shot output; omit to launch the interactive TUI.
    #[command(subcommand)]
    command: Option<Command>,

    /// Agent filter applied to TUI mode when no subcommand is specified.
    #[arg(long, value_enum, default_value_t = AgentFilter::All)]
    agent: AgentFilter,

    /// Background auto-refresh interval in seconds for TUI mode.
    #[arg(long, default_value_t = 60)]
    refresh_secs: u64,
}

/// One-shot report subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Show daily token, cost, and speed breakdown.
    Daily(DailyArgs),

    /// Show hourly token, cost, and speed breakdown.
    Hourly(AgentArgs),

    /// Show overall usage and speed summary.
    Summary(AgentArgs),
}

/// Common `--agent` flag for subcommands that do not take a date range.
#[derive(Debug, Args)]
struct AgentArgs {
    /// Target agent source to query.
    #[arg(long, value_enum, default_value_t = AgentFilter::All)]
    agent: AgentFilter,
}

/// Arguments for the `daily` subcommand.
#[derive(Debug, Args)]
struct DailyArgs {
    /// Target agent source to query.
    #[arg(long, value_enum, default_value_t = AgentFilter::All)]
    agent: AgentFilter,

    /// Show the last N days (inclusive of today).
    #[arg(long, conflicts_with_all = ["start", "end"])]
    last: Option<u32>,

    /// Inclusive start date (YYYY-MM-DD).
    #[arg(long, requires = "end")]
    start: Option<String>,

    /// Inclusive end date (YYYY-MM-DD).
    #[arg(long, requires = "start")]
    end: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => {
            println!(
                "TUI mode (not yet implemented): agent={:?}, refresh={}s",
                cli.agent, cli.refresh_secs
            );
        }
        Some(Command::Daily(args)) => {
            println!("daily report (not yet implemented): {:?}", args);
        }
        Some(Command::Hourly(args)) => {
            println!("hourly report (not yet implemented): {:?}", args);
        }
        Some(Command::Summary(args)) => {
            println!("summary report (not yet implemented): {:?}", args);
        }
    }

    Ok(())
}
