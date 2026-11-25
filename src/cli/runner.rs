//! 单次 CLI 命令执行入口：调度时间维度报表与会话查询。

use anyhow::Result;
use token_counter::adapters::{self, ScanQuery};
use token_counter::core::pricing::PricingCatalog;
use token_counter::core::time::{Granularity, TimeRange};
use token_counter::stats::{aggregate, session};

use super::args::{Command, ReportArgs, SessionArgs};
use super::spinner::Spinner;
use super::{report_render, session_render};

/// 未指定 `-n` 时的默认分桶数 / 会话条数。
const DEFAULT_HOURS: u32 = 24;
const DEFAULT_DAYS: u32 = 14;
const DEFAULT_WEEKS: u32 = 12;
const DEFAULT_MONTHS: u32 = 12;
const DEFAULT_SESSIONS: usize = 20;

/// 执行单次 CLI 子命令（`Tui` 已由 `main` 路由，此处忽略）。
pub fn run(command: Command, catalog: &PricingCatalog) -> Result<()> {
    match command {
        Command::Hourly(args) => run_report(Granularity::Hourly, args, DEFAULT_HOURS, catalog),
        Command::Daily(args) => run_report(Granularity::Daily, args, DEFAULT_DAYS, catalog),
        Command::Weekly(args) => run_report(Granularity::Weekly, args, DEFAULT_WEEKS, catalog),
        Command::Monthly(args) => run_report(Granularity::Monthly, args, DEFAULT_MONTHS, catalog),
        Command::Session(args) => run_session(args, catalog),
        Command::Tui => Ok(()),
    }
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

    println!("{}", report_render::render_report(&report));
    Ok(())
}

/// 执行会话列表或单会话 Turn 明细查询（支持 `all` 与单个具体 Agent）。
fn run_session(args: SessionArgs, catalog: &PricingCatalog) -> Result<()> {
    let agents = args.agent.agents();

    let loading = Spinner::start("Loading...");
    let output = match args.id {
        Some(ref id_prefix) => {
            let query = ScanQuery::SessionPrefix(id_prefix.clone());
            let events = adapters::scan_agents_query(&agents, &query, catalog)?;
            let detail = session::find_session_detail(&events, id_prefix)?;
            session_render::render_session_detail(&detail)
        }
        None => {
            let limit = args.last.unwrap_or(DEFAULT_SESSIONS);
            let query = ScanQuery::RecentSessions { limit };
            let events = adapters::scan_agents_query(&agents, &query, catalog)?;
            let list = session::summarize_sessions(&events);
            session_render::render_session_list(&list, limit)
        }
    };
    loading.stop();

    println!("{}", output);
    Ok(())
}
