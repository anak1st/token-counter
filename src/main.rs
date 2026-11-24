//! token-counter 可执行程序入口。

mod cli;
mod spinner;

use anyhow::Result;
use clap::Parser;
use token_counter::adapters::{self, ScanQuery};
use token_counter::aggregate;
use token_counter::pricing::PricingCatalog;
use token_counter::render;
use token_counter::session;
use token_counter::time::{Granularity, TimeRange};

use crate::cli::{Cli, Command, ReportArgs, SessionArgs};
use crate::spinner::Spinner;

/// 未指定 `-n` 时的默认分桶数 / 会话条数。
const DEFAULT_HOURS: u32 = 24;
const DEFAULT_DAYS: u32 = 14;
const DEFAULT_WEEKS: u32 = 12;
const DEFAULT_MONTHS: u32 = 12;
const DEFAULT_SESSIONS: usize = 20;

/// 交互式 TUI 模式入口占位（Step 4 实现）。
fn run_tui(catalog: &PricingCatalog) -> Result<()> {
    println!(
        "TUI mode (not yet implemented): models={}",
        catalog.model_count()
    );
    Ok(())
}

/// 执行单次时间维度统计并将纯文本对齐表格打印到标准输出。
fn run_report(
    granularity: Granularity,
    args: ReportArgs,
    default_n: u32,
    catalog: &PricingCatalog,
) -> Result<()> {
    let agents = args.agent.agents();
    let count = args.last.unwrap_or(default_n);
    let range = TimeRange::last_n(granularity, count)?;

    let loading = Spinner::start("Loading...");
    let events = adapters::scan_agents(&agents, range, catalog)?;
    let report = aggregate::aggregate(&events, granularity);
    loading.stop();

    println!("{}", render::render_report(&report));
    Ok(())
}

/// 执行单 Agent 的会话列表或单会话 Turn 明细查询。
fn run_session(args: SessionArgs, catalog: &PricingCatalog) -> Result<()> {
    let agent = args.agent.into_agent();

    let loading = Spinner::start("Loading...");
    let output = match args.id {
        Some(ref id_prefix) => {
            let query = ScanQuery::SessionPrefix(id_prefix.clone());
            let events = adapters::scan_agent(agent, &query, catalog)?;
            let detail = session::find_session_detail(&events, id_prefix)?;
            session::render_session_detail(&detail)
        }
        None => {
            let limit = args.last.unwrap_or(DEFAULT_SESSIONS);
            let query = ScanQuery::RecentSessions { limit };
            let events = adapters::scan_agent(agent, &query, catalog)?;
            let list = session::summarize_sessions(&events);
            session::render_session_list(&list, limit)
        }
    };
    loading.stop();

    println!("{}", output);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (catalog, _source) = PricingCatalog::resolve(None)?;

    match cli.command {
        None | Some(Command::Tui) => run_tui(&catalog),
        Some(Command::Hourly(args)) => {
            run_report(Granularity::Hourly, args, DEFAULT_HOURS, &catalog)
        }
        Some(Command::Daily(args)) => run_report(Granularity::Daily, args, DEFAULT_DAYS, &catalog),
        Some(Command::Weekly(args)) => {
            run_report(Granularity::Weekly, args, DEFAULT_WEEKS, &catalog)
        }
        Some(Command::Monthly(args)) => {
            run_report(Granularity::Monthly, args, DEFAULT_MONTHS, &catalog)
        }
        Some(Command::Session(args)) => run_session(args, &catalog),
    }
}
