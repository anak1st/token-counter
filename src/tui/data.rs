//! TUI 数据快照构建与梯度扫描支持。
//!
//! 后台线程采用「首屏 30 天 + 历史补全 + 60s 仅刷今天（1 天）」加载策略：
//! - 启动先扫最近 30 天 `[initial_cutoff_ms, ∞)` 快速点亮首屏；
//! - 紧接着补扫更早历史，并将今日 00:00 之前的所有事件冻结驻留内存；
//! - 之后每 60s（或按 `r`）只重扫今日 `[refresh_cutoff_ms, ∞)` 再拼合冻结历史。
//!
//! 每次构建快照时同步预聚合「全量（All）」与「各单个 Agent」视角，
//! 保证用户在任意 Tab 切换 Agent 筛选时零延迟响应。

use std::collections::HashMap;

use anyhow::Result;
use jiff::ZonedDateTime;
use token_counter::adapters;
use token_counter::core::model::{Agent, UsageEvent};
use token_counter::core::pricing::PricingCatalog;
use token_counter::core::time::{Granularity, TimeRange};
use token_counter::report::aggregate::{self, BucketRow, Report};
use token_counter::report::session::{self, SessionDetail};

/// 启动首屏快速扫描天数（快速点亮首屏后再后台补全更早历史）。
pub const INITIAL_WINDOW_DAYS: u32 = 30;
/// 后续定时/手动增量刷新窗口天数（仅重扫今天 00:00 起的数据）。
pub const REFRESH_WINDOW_DAYS: u32 = 1;

/// 单条 `agent · provider · model` 汇总行（供 `Overview` 与时间维度 Tab 共用）。
#[derive(Debug, Clone)]
pub struct ModelRow {
    pub agent: Agent,
    pub provider: String,
    pub model: String,
    pub metrics: BucketRow,
}

impl ModelRow {
    /// 统一展示名：`agent · provider · model`。
    pub fn display_name(&self) -> String {
        format!("{} · {} · {}", self.agent.id(), self.provider, self.model)
    }
}

/// 单个时间粒度（`Hour` / `Day` / `Week` / `Month`）的倒序汇总与各分桶模型明细。
#[derive(Debug, Clone)]
pub struct PeriodData {
    /// 按时间倒序排列的分桶汇总行（最新在最上面）。
    pub rows: Vec<BucketRow>,
    /// 分桶标签 -> 该分桶内各 `agent · provider · model` 明细（按总 token 降序）。
    pub breakdowns: HashMap<String, Vec<ModelRow>>,
}

/// 附带所属 Agent 标识的单条会话详情（供 `Session` Tab 在 `All` 与单 Agent 模式下共用）。
#[derive(Debug, Clone)]
pub struct SessionItem {
    pub agent: Agent,
    pub detail: SessionDetail,
}

/// 单个筛选视角（`All` 或某个具体 Agent）的完整聚合数据。
#[derive(Debug, Clone)]
pub struct ViewSnapshot {
    /// 合并总计（供 `Overview` 顶部 KPI 条使用）。
    pub overall_total: BucketRow,
    /// 按小时倒序数据（`Hour` Tab 与首页 `Hours` 柱状图共用）。
    pub hourly: PeriodData,
    /// 按天倒序数据（`Day` Tab 与首页 `Days` 柱状图共用）。
    pub daily: PeriodData,
    /// 按周倒序数据（`Week` Tab）。
    pub weekly: PeriodData,
    /// 按月倒序数据（`Month` Tab）。
    pub monthly: PeriodData,
    /// 各 `agent · provider · model` 汇总（按总 token 降序）。
    pub model_rows: Vec<ModelRow>,
    /// 会话与 Turn 明细列表（按最后活跃时间倒序，每个会话内 Turn 按时间升序）。
    pub sessions: Vec<SessionItem>,
    /// 未知模型列表（`agent · provider · model`）。
    pub unknown_models: Vec<String>,
}

/// 后台线程一次性构建完毕的 TUI 快照。
#[derive(Debug, Clone)]
pub struct TuiSnapshot {
    /// 全量视角（`All` 模式）。
    pub all: ViewSnapshot,
    /// 按单个 Agent 预聚合的视角（覆盖 `Agent::ALL` 中每个 Agent）。
    pub by_agent: HashMap<Agent, ViewSnapshot>,
    /// 为 true 表示当前为首阶段 30 天快速快照，后台仍在补全更早历史。
    pub is_partial: bool,
    /// 本次快照生成时刻（本地 `HH:MM:SS`）。
    pub updated_at: String,
    /// 本次后台加载耗时（毫秒）。
    pub load_ms: u64,
}

impl TuiSnapshot {
    /// 按筛选条件返回对应聚合视角（`None` 表示 `All`）。
    pub fn view(&self, filter: Option<Agent>) -> &ViewSnapshot {
        match filter {
            None => &self.all,
            Some(agent) => self.by_agent.get(&agent).unwrap_or(&self.all),
        }
    }
}

/// 计算最近 `days` 天窗口的起点时间戳（本地时区 `00:00`）。
pub fn window_cutoff_ms(days: u32) -> Result<i64> {
    let range = TimeRange::last_n(Granularity::Daily, days)?;
    Ok(range.start_ms)
}

/// 扫描全部内置 Agent 在指定半开时间窗内的事件。
pub fn scan_range(range: TimeRange, catalog: &PricingCatalog) -> Result<Vec<UsageEvent>> {
    adapters::scan_agents(&Agent::ALL, range, catalog)
}

/// 将事件列表聚合成完整的 `TuiSnapshot`（纯内存计算，零 I/O）。
pub fn build_snapshot(events: &[UsageEvent], is_partial: bool, load_ms: u64) -> TuiSnapshot {
    let now = ZonedDateTime::now();
    let updated_at = format!(
        "{:02}:{:02}:{:02}",
        now.hour(),
        now.minute(),
        now.second()
    );

    // 按 Agent 分桶一次，分别构建单 Agent 视角与全量合并会话列表
    let mut events_by_agent: HashMap<Agent, Vec<UsageEvent>> =
        HashMap::with_capacity(Agent::ALL.len());
    for agent in Agent::ALL {
        events_by_agent.insert(agent, Vec::new());
    }
    for event in events {
        if let Some(bucket) = events_by_agent.get_mut(&event.agent) {
            bucket.push(event.clone());
        }
    }

    let mut by_agent: HashMap<Agent, ViewSnapshot> = HashMap::with_capacity(Agent::ALL.len());
    let mut all_sessions: Vec<SessionItem> = Vec::new();

    for agent in Agent::ALL {
        let agent_events = events_by_agent.get(&agent).map(Vec::as_slice).unwrap_or(&[]);
        let agent_sessions: Vec<SessionItem> =
            session::summarize_sessions_with_turns(agent_events, agent)
                .into_iter()
                .map(|detail| SessionItem { agent, detail })
                .collect();
        all_sessions.extend(agent_sessions.iter().cloned());
        by_agent.insert(agent, build_view_snapshot(agent_events, agent_sessions));
    }

    all_sessions.sort_by(|a, b| {
        b.detail
            .summary
            .last_ts_ms
            .cmp(&a.detail.summary.last_ts_ms)
            .then_with(|| a.agent.id().cmp(b.agent.id()))
            .then_with(|| a.detail.summary.session_id.cmp(&b.detail.summary.session_id))
    });

    let all = build_view_snapshot(events, all_sessions);

    TuiSnapshot {
        all,
        by_agent,
        is_partial,
        updated_at,
        load_ms,
    }
}

/// 将一组事件聚合为单个视角的 `ViewSnapshot`。
fn build_view_snapshot(events: &[UsageEvent], sessions: Vec<SessionItem>) -> ViewSnapshot {
    let daily_report = aggregate::aggregate(events, Granularity::Daily);
    let hourly_report = aggregate::aggregate(events, Granularity::Hourly);
    let weekly_report = aggregate::aggregate(events, Granularity::Weekly);
    let monthly_report = aggregate::aggregate(events, Granularity::Monthly);

    let mut model_rows: Vec<ModelRow> = Vec::with_capacity(daily_report.groups.len());
    for group in &daily_report.groups {
        model_rows.push(ModelRow {
            agent: group.agent,
            provider: group.provider.clone(),
            model: group.model.clone(),
            metrics: group.total.clone(),
        });
    }
    sort_by_tokens_desc(&mut model_rows);

    let overall_total = daily_report.overall.total.clone();
    // 将 `agent/provider/model` 统一展示为 `agent · provider · model`
    let unknown_models: Vec<String> = daily_report
        .unknown_models
        .iter()
        .map(|raw| raw.replace('/', " · "))
        .collect();

    let daily = report_into_period(daily_report);
    let hourly = report_into_period(hourly_report);
    let weekly = report_into_period(weekly_report);
    let monthly = report_into_period(monthly_report);

    ViewSnapshot {
        overall_total,
        hourly,
        daily,
        weekly,
        monthly,
        model_rows,
        sessions,
        unknown_models,
    }
}

/// 将单次 `Report` 转换为时间倒序的 `PeriodData`（每个分桶内按 token 降序）。
fn report_into_period(report: Report) -> PeriodData {
    let mut rows = report.overall.rows;
    rows.reverse();

    let mut breakdowns: HashMap<String, Vec<ModelRow>> = HashMap::new();
    for group in report.groups {
        for bucket_row in group.rows {
            breakdowns
                .entry(bucket_row.label.clone())
                .or_default()
                .push(ModelRow {
                    agent: group.agent,
                    provider: group.provider.clone(),
                    model: group.model.clone(),
                    metrics: bucket_row,
                });
        }
    }

    for items in breakdowns.values_mut() {
        sort_by_tokens_desc(items);
    }

    PeriodData { rows, breakdowns }
}

/// 按总 token 使用量降序排列模型行。
fn sort_by_tokens_desc(rows: &mut [ModelRow]) {
    rows.sort_by(|a, b| {
        b.metrics
            .total_tokens()
            .cmp(&a.metrics.total_tokens())
            .then_with(|| a.display_name().cmp(&b.display_name()))
    });
}
