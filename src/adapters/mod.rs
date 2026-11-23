//! 内置三个 Agent（`opencode`、`dsh`、`agy`）的数据源 adapter 分发层。
//!
//! 所有 adapter 均严格只读打开本地数据源，并在入口完成模型归一与官方美元价结算。

pub mod agy;
pub mod dsh;
pub mod opencode;

use anyhow::Result;

use crate::model::{Agent, UsageEvent};
use crate::pricing::PricingCatalog;
use crate::time::TimeRange;

/// 扫描单个 Agent 在 `[range.start_ms, range.end_ms)` 内的全部请求事件。
pub fn scan_agent(
    agent: Agent,
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    match agent {
        Agent::Opencode => opencode::scan(range, catalog),
        Agent::Dsh => dsh::scan(range, catalog),
        Agent::Agy => agy::scan(range, catalog),
    }
}

/// 扫描多个 Agent 并合并其事件列表。
pub fn scan_agents(
    agents: &[Agent],
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let mut events = Vec::new();
    for &agent in agents {
        let mut part = scan_agent(agent, range, catalog)?;
        events.append(&mut part);
    }
    Ok(events)
}
