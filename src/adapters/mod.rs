//! 内置三个 Agent（`opencode`、`dsh`、`agy`）的数据源 adapter 统一分发层。
//!
//! 所有 adapter 均通过统一的 `ScanQuery` 协议接收查询意图，严格只读打开本地数据，
//! 并在入口完成模型归一与官方美元价结算。

pub mod agy;
pub mod dsh;
pub mod opencode;

use anyhow::Result;

use crate::core::model::{Agent, UsageEvent};
use crate::core::pricing::PricingCatalog;
use crate::core::time::TimeRange;

/// 统一的适配器查询意图，供时间报表与会话查询共用。
#[derive(Debug, Clone)]
pub enum ScanQuery {
    /// 按半开时间窗口查询全部事件（用于 `hourly` / `daily` / `weekly` / `monthly`）。
    Range(TimeRange),
    /// 查询最近活跃的 `limit` 个会话（适配器可据此按修改时间剪枝）。
    RecentSessions { limit: usize },
    /// 按会话 ID 前缀查询单个会话（适配器可据此按文件名/目录名剪枝）。
    SessionPrefix(String),
}

impl ScanQuery {
    /// 返回本次查询对应的时间过滤窗口。
    pub fn time_range(&self) -> TimeRange {
        match self {
            ScanQuery::Range(range) => *range,
            ScanQuery::RecentSessions { .. } | ScanQuery::SessionPrefix(_) => TimeRange::ALL,
        }
    }
}

/// 按统一查询协议扫描单个 Agent。
pub fn scan_agent(
    agent: Agent,
    query: &ScanQuery,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    match agent {
        Agent::Opencode => opencode::scan(query.time_range(), catalog),
        Agent::Dsh => dsh::scan(query, catalog),
        Agent::Agy => agy::scan(query.time_range(), catalog),
    }
}

/// 按时间范围扫描多个 Agent 并合并结果。
pub fn scan_agents(
    agents: &[Agent],
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let query = ScanQuery::Range(range);
    let mut events = Vec::new();
    for &agent in agents {
        let mut part = scan_agent(agent, &query, catalog)?;
        events.append(&mut part);
    }
    Ok(events)
}
