//! TUI 数据快照构建与梯度扫描支持。
//!
//! 后台线程采用「30 天热窗 + 历史冷窗」两段式加载：
//! - 热窗 `[cutoff_ms, ∞)`：启动先扫以毫秒级出首屏，之后每 60s 只重扫热窗；
//! - 冷窗 `[0, cutoff_ms)`：后台静默补扫一次并驻留内存，与热窗无缝拼合为完整全量。

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use jiff::{ToSpan, ZonedDateTime};
use token_counter::adapters;
use token_counter::core::model::{Agent, UsageEvent};
use token_counter::core::pricing::PricingCatalog;
use token_counter::core::time::{Granularity, TimeRange};
use token_counter::report::aggregate::{self, BucketRow, Report};
use token_counter::report::session::{self, SessionDetail};

/// `Overview` 堆叠柱状图展示的最近日历天数，同时作为热窗默认天数。
pub const HOT_WINDOW_DAYS: u32 = 30;
pub const CHART_DAYS: usize = 30;

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

/// 堆叠柱状图中单根柱子的单个模型分段。
#[derive(Debug, Clone)]
pub struct ChartSegment {
    pub model: String,
    pub tokens: u64,
}

/// 堆叠柱状图中的一天（含零用量空白日，按时间从旧到新排列）。
#[derive(Debug, Clone)]
pub struct ChartDay {
    /// 简短横轴标签 `MM-DD`。
    pub short_label: String,
    pub total_tokens: u64,
    pub segments: Vec<ChartSegment>,
}

/// 后台线程一次性构建完毕的 TUI 快照。
#[derive(Debug, Clone)]
pub struct TuiSnapshot {
    /// 全量合并总计（供 `Overview` 顶部 KPI 条使用）。
    pub overall_total: BucketRow,
    /// 按小时倒序数据（`Hour` Tab）。
    pub hourly: PeriodData,
    /// 按天倒序数据（`Day` Tab）。
    pub daily: PeriodData,
    /// 按周倒序数据（`Week` Tab）。
    pub weekly: PeriodData,
    /// 按月倒序数据（`Month` Tab）。
    pub monthly: PeriodData,
    /// 按 Agent 分组的会话与 Turn 明细列表（会话按最后活跃时间倒序，Turn 按时间升序）。
    pub sessions_by_agent: HashMap<Agent, Vec<SessionDetail>>,
    /// 全量各 `agent · provider · model` 汇总（按总 token 降序）。
    pub model_rows: Vec<ModelRow>,
    /// 最近 30 个日历日的堆叠柱数据（左旧右新）。
    pub chart_days: Vec<ChartDay>,
    /// 未知模型列表（`agent/provider/model`）。
    pub unknown_models: Vec<String>,
    /// 为 true 表示当前为首阶段 30 天快速快照，后台仍在补全更早历史。
    pub is_partial: bool,
    /// 本次快照生成时刻（本地 `HH:MM:SS`）。
    pub updated_at: String,
    /// 本次后台加载耗时（毫秒）。
    pub load_ms: u64,
}

/// 计算热窗起点时间戳（本地时区 30 天前 00:00）。
pub fn hot_window_cutoff_ms() -> Result<i64> {
    let range = TimeRange::last_n(Granularity::Daily, HOT_WINDOW_DAYS)?;
    Ok(range.start_ms)
}

/// 扫描全部内置 Agent 在指定半开时间窗内的事件。
pub fn scan_range(range: TimeRange, catalog: &PricingCatalog) -> Result<Vec<UsageEvent>> {
    adapters::scan_agents(&Agent::ALL, range, catalog)
}

/// 将事件列表聚合成完整的 `TuiSnapshot`（纯内存计算，零 I/O）。
pub fn build_snapshot(events: &[UsageEvent], is_partial: bool, load_ms: u64) -> TuiSnapshot {
    let daily_report = aggregate::aggregate(events, Granularity::Daily);
    let hourly_report = aggregate::aggregate(events, Granularity::Hourly);
    let weekly_report = aggregate::aggregate(events, Granularity::Weekly);
    let monthly_report = aggregate::aggregate(events, Granularity::Monthly);

    // 从日聚合中顺手提取堆叠柱分段与全量模型汇总，避免重复遍历
    let mut chart_by_day: HashMap<String, BTreeMap<String, u64>> = HashMap::new();
    let mut model_rows: Vec<ModelRow> = Vec::with_capacity(daily_report.groups.len());

    for group in &daily_report.groups {
        for day_row in &group.rows {
            let tokens = day_row.total_tokens();
            if tokens > 0 {
                let model_map = chart_by_day.entry(day_row.label.clone()).or_default();
                let entry = model_map.entry(group.model.clone()).or_insert(0);
                *entry = entry.saturating_add(tokens);
            }
        }
        model_rows.push(ModelRow {
            agent: group.agent,
            provider: group.provider.clone(),
            model: group.model.clone(),
            metrics: group.total.clone(),
        });
    }
    sort_by_tokens_desc(&mut model_rows);

    let overall_total = daily_report.overall.total.clone();
    let unknown_models = daily_report.unknown_models.clone();

    let daily = report_into_period(daily_report);
    let hourly = report_into_period(hourly_report);
    let weekly = report_into_period(weekly_report);
    let monthly = report_into_period(monthly_report);

    let mut sessions_by_agent: HashMap<Agent, Vec<SessionDetail>> =
        HashMap::with_capacity(Agent::ALL.len());
    for agent in Agent::ALL {
        let details = session::summarize_sessions_with_turns(events, agent);
        sessions_by_agent.insert(agent, details);
    }

    let now = ZonedDateTime::now();
    let chart_days = build_recent_chart_days(&now, &chart_by_day);
    let updated_at = format!(
        "{:02}:{:02}:{:02}",
        now.hour(),
        now.minute(),
        now.second()
    );

    TuiSnapshot {
        overall_total,
        hourly,
        daily,
        weekly,
        monthly,
        sessions_by_agent,
        model_rows,
        chart_days,
        unknown_models,
        is_partial,
        updated_at,
        load_ms,
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

/// 生成最近 `CHART_DAYS` 个连续日历日的堆叠柱数据（左旧右新，空白日补 0）。
fn build_recent_chart_days(
    now: &ZonedDateTime,
    by_day: &HashMap<String, BTreeMap<String, u64>>,
) -> Vec<ChartDay> {
    let today = now.date();
    let mut days = Vec::with_capacity(CHART_DAYS);

    for offset in (0..CHART_DAYS).rev() {
        let date = match today.checked_sub((offset as i64).days()) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let key = format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day());
        let short_label = format!("{:02}-{:02}", date.month(), date.day());

        let mut segments = Vec::new();
        let mut total_tokens = 0u64;

        if let Some(model_map) = by_day.get(&key) {
            for (model, &tokens) in model_map {
                total_tokens = total_tokens.saturating_add(tokens);
                segments.push(ChartSegment {
                    model: model.clone(),
                    tokens,
                });
            }
        }

        // 分段按 token 升序排列，大头在顶部便于阅读
        segments.sort_by(|a, b| a.tokens.cmp(&b.tokens).then_with(|| a.model.cmp(&b.model)));

        days.push(ChartDay {
            short_label,
            total_tokens,
            segments,
        });
    }

    days
}
