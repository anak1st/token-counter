//! 命令行参数定义。
//!
//! - `token-counter` / `token-counter tui`：进入交互式 TUI（不接收额外参数）
//! - `hourly` / `daily` / `weekly` / `monthly`：单次报表，共用 `-a/--agent` 与 `-n`
//!
//! 注意：`clap` 帮助文案通过显式英文 `about`/`help` 属性提供，保证终端输出全英文。

use clap::{Args, Parser, Subcommand, ValueEnum};
use token_counter::model::Agent;

// 顶层 CLI 定义
#[derive(Debug, Parser)]
#[command(
    name = "token-counter",
    version,
    about = "Token usage, cost, and speed analytics for AI coding agents"
)]
pub struct Cli {
    // 子命令；缺省时直接启动 TUI
    #[command(subcommand)]
    pub command: Option<Command>,
}

// 支持的子命令集合
#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Show hourly breakdown (default: last 24 hours)")]
    Hourly(ReportArgs),

    #[command(about = "Show daily breakdown (default: last 14 days)")]
    Daily(ReportArgs),

    #[command(about = "Show weekly breakdown (default: last 12 weeks)")]
    Weekly(ReportArgs),

    #[command(about = "Show monthly breakdown (default: last 12 months)")]
    Monthly(ReportArgs),

    #[command(about = "Launch interactive TUI dashboard")]
    Tui,
}

// 四个单次报表子命令共用的参数
#[derive(Debug, Args)]
pub struct ReportArgs {
    #[arg(
        short = 'a',
        long,
        value_enum,
        default_value_t = AgentArg::All,
        help = "Agent to query"
    )]
    pub agent: AgentArg,

    #[arg(
        short = 'n',
        value_name = "N",
        help = "Number of recent buckets (default: 24h / 14d / 12w / 12m)"
    )]
    pub last: Option<u32>,
}

// `-a / --agent` 允许的取值
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentArg {
    All,
    Opencode,
    Dsh,
    Agy,
}

impl AgentArg {
    /// 将 CLI 选项展开为实际待扫描的 Agent 列表。
    pub fn agents(self) -> Vec<Agent> {
        match self {
            AgentArg::All => Agent::ALL.to_vec(),
            AgentArg::Opencode => vec![Agent::Opencode],
            AgentArg::Dsh => vec![Agent::Dsh],
            AgentArg::Agy => vec![Agent::Agy],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tui_and_report_commands() {
        assert!(Cli::try_parse_from(["token-counter"])
            .unwrap()
            .command
            .is_none());
        assert!(matches!(
            Cli::try_parse_from(["token-counter", "tui"])
                .unwrap()
                .command,
            Some(Command::Tui)
        ));

        let daily =
            Cli::try_parse_from(["token-counter", "daily", "-a", "dsh", "-n", "14"]).unwrap();
        match daily.command {
            Some(Command::Daily(args)) => {
                assert_eq!(args.agent, AgentArg::Dsh);
                assert_eq!(args.last, Some(14));
            }
            other => panic!("expected Daily, got {:?}", other),
        }

        // TUI 模式与顶层不接受 --agent 参数
        assert!(Cli::try_parse_from(["token-counter", "-a", "dsh"]).is_err());
        assert!(Cli::try_parse_from(["token-counter", "tui", "-a", "dsh"]).is_err());
    }
}
