//! token-counter 二进制入口：解析命令行并路由到 `tui` 或 `cli`。

mod cli;
mod tui;

use anyhow::Result;
use clap::Parser;
use token_counter::core::pricing::PricingCatalog;

use crate::cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (catalog, _) = PricingCatalog::resolve(None)?;

    match cli.command {
        None | Some(Command::Tui) => tui::run(catalog),
        Some(cmd) => cli::run(cmd, &catalog),
    }
}
