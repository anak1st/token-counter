//! token-counter 可执行程序入口。

mod cli;

use anyhow::Result;
use clap::Parser;
use token_counter::adapters;
use token_counter::aggregate;
use token_counter::pricing::PricingCatalog;
use token_counter::render;
use token_counter::time::{Granularity, TimeRange};

use crate::cli::{Cli, Command, ReportArgs};

/// 未指定 `-n` 时的默认分桶数。
const DEFAULT_HOURS: u32 = 24;
const DEFAULT_DAYS: u32 = 14;
const DEFAULT_WEEKS: u32 = 12;
const DEFAULT_MONTHS: u32 = 12;

/// 交互式 TUI 模式入口占位（Step 4 实现）。
fn run_tui(catalog: &PricingCatalog) -> Result<()> {
    println!("TUI mode (not yet implemented): models={}", catalog.model_count());
    Ok(())
}

/// 执行单次统计查询并将纯文本对齐表格打印到标准输出。
fn run_report(
    granularity: Granularity,
    args: ReportArgs,
    default_n: u32,
    catalog: &PricingCatalog,
) -> Result<()> {
    let agents = args.agent.agents();
    let count = args.last.unwrap_or(default_n);
    let range = TimeRange::last_n(granularity, count)?;
    let events = adapters::scan_agents(&agents, range, catalog)?;
    let report = aggregate::aggregate(&events, granularity);
    println!("{}", render::render_report(&report));
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
    }
}
