//! TUI 数据快照构建。
//!
//! 在后台线程扫描全部内置 Agent（`opencode`、`dsh`、`agy`），复用核心聚合结果，
//! 生成供 `Overview` 与 `Day` 两个视图直接渲染的不可变快照。

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use anyhow::Result;
use jiff::{ToSpan, ZonedDateTime};
use token_counter::adapters;
use token_counter::core::model::Agent;
use token_counter::core::pricing::PricingCatalog;
use token_counter::core::time::{Granularity, TimeRange};
use token_counter::report::aggregate::{self, BucketRow};

/// `Overview` 堆叠柱状图展示的最近日历天数。
pub const CHART_DAYS: usize = 30;

/// 单条 `agent · provider · model` 汇总行（供 `Overview` 与 `Day` 共用）。
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

/// 后台线程一次性构建完毕的 TUI 全量快照。
#[derive(Debug, Clone)]
pub struct TuiSnapshot {
    /// 全量合并总计（供 `Overview` 顶部 KPI 条使用）。
    pub overall_total: BucketRow,
    /// 按时间倒序排列的每日汇总行（最新日期在最上面）。
    pub daily_rows: Vec<BucketRow>,
    /// 每日明细映射：`YYYY-MM-DD` -> 当天各 `agent · provider · model`（按 token 降序）。
    pub daily_breakdowns: HashMap<String, Vec<ModelRow>>,
    /// 全量各 `agent · provider · model` 汇总（按 token 降序）。
    pub model_rows: Vec<ModelRow>,
    /// 最近 30 个日历日的堆叠柱数据（左旧右新）。
    pub chart_days: Vec<ChartDay>,
    /// 未知模型列表（`agent/provider/model`）。
    pub unknown_models: Vec<String>,
    /// 本次快照生成时刻（本地 `HH:MM:SS`）。
    pub updated_at: String,
    /// 本次后台加载耗时（毫秒）。
    pub load_ms: u64,
}

/// 扫描全部 Agent 并构建一份新的 `TuiSnapshot`。
pub fn load_snapshot(catalog: &PricingCatalog) -> Result<TuiSnapshot> {
    let started = Instant::now();
    let events = adapters::scan_agents(&Agent::ALL, TimeRange::ALL, catalog)?;
    let report = aggregate::aggregate(&events, Granularity::Daily);

    // 每日汇总行改为时间倒序（最新日期排在最上面）
    let mut daily_rows = report.overall.rows;
    daily_rows.reverse();

    // 构建全量模型汇总表与按天拆分的明细表
    let mut model_rows: Vec<ModelRow> = Vec::with_capacity(report.groups.len());
    let mut daily_breakdowns: HashMap<String, Vec<ModelRow>> = HashMap::new();
    // 按 (日期, 模型) 聚合堆叠柱分段（同一模型跨渠道/Agent 在柱图中归并同色）
    let mut chart_by_day: HashMap<String, BTreeMap<String, u64>> = HashMap::new();

    for group in report.groups {
        for day_row in &group.rows {
            let date_key = day_row.label.clone();
            let day_tokens = day_row.total_tokens();
            if day_tokens > 0 {
                let model_map = chart_by_day.entry(date_key.clone()).or_default();
                let entry = model_map.entry(group.model.clone()).or_insert(0);
                *entry = entry.saturating_add(day_tokens);
            }

            daily_breakdowns.entry(date_key).or_default().push(ModelRow {
                agent: group.agent,
                provider: group.provider.clone(),
                model: group.model.clone(),
                metrics: day_row.clone(),
            });
        }

        model_rows.push(ModelRow {
            agent: group.agent,
            provider: group.provider,
            model: group.model,
            metrics: group.total,
        });
    }

    // 首页模型表与每日明细表统一按总 token 使用量降序，同量按名称稳定排序
    sort_by_tokens_desc(&mut model_rows);
    for rows in daily_breakdowns.values_mut() {
        sort_by_tokens_desc(rows);
    }

    let now = ZonedDateTime::now();
    let chart_days = build_recent_chart_days(&now, &chart_by_day);
    let updated_at = format!(
        "{:02}:{:02}:{:02}",
        now.hour(),
        now.minute(),
        now.second()
    );

    Ok(TuiSnapshot {
        overall_total: report.overall.total,
        daily_rows,
        daily_breakdowns,
        model_rows,
        chart_days,
        unknown_models: report.unknown_models,
        updated_at,
        load_ms: started.elapsed().as_millis() as u64,
    })
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
