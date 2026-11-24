//! 归一化事件的内存聚合。
//!
//! 按 `(provider, 统一模型名, 时间分桶)` 三键聚合：
//! - 有价模型折入 `overall` 总计表以及各自的 `(provider, model)` 分组表；
//! - 未知模型（`models.json` 中查不到单价）不出表格，汇总到 `unknown_models` 末尾提示。

use std::collections::{BTreeMap, BTreeSet};

use crate::core::model::{Agent, UsageEvent};
use crate::core::pricing::ModelPricing;
use crate::core::time::{bucket_key, Granularity};

/// 单个时间分桶（或 `Total` 合计行）的聚合指标。
#[derive(Debug, Clone, PartialEq)]
pub struct BucketRow {
    /// 分桶标签（如 `2026-11-23`、`2026-11-23 16:00`、`2026-W48`、`2026-11` 或 `Total`）。
    pub label: String,
    /// 本分桶内的模型请求次数。
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost_usd: f64,
    pub ttft_sum_ms: u64,
    pub ttft_count: u64,
    /// 带有效时长样本的纯解码总耗时（毫秒）。
    pub gen_ms: u64,
    /// 与 `gen_ms` 严格同源的 `output + reasoning` 累计值，用于计算解码速度。
    pub speed_tokens: u64,
}

impl BucketRow {
    /// 创建空白聚合行。
    pub fn empty(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            requests: 0,
            input: 0,
            output: 0,
            reasoning: 0,
            cache_read: 0,
            cache_write: 0,
            cost_usd: 0.0,
            ttft_sum_ms: 0,
            ttft_count: 0,
            gen_ms: 0,
            speed_tokens: 0,
        }
    }

    /// 全部 token 桶的总和。
    pub fn total_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    /// 缓存相关 token（`cache_read + cache_write`）。
    pub fn cache_tokens(&self) -> u64 {
        self.cache_read.saturating_add(self.cache_write)
    }

    /// 缓存占比（0.0..=100.0），总 token 为 0 时返回 `None`。
    pub fn cache_percent(&self) -> Option<f64> {
        let total = self.total_tokens();
        if total == 0 {
            return None;
        }
        Some((self.cache_tokens() as f64 / total as f64) * 100.0)
    }

    /// 平均首字延迟（秒），无样本时返回 `None`。
    pub fn avg_ttft_secs(&self) -> Option<f64> {
        if self.ttft_count == 0 {
            return None;
        }
        Some((self.ttft_sum_ms as f64 / self.ttft_count as f64) / 1000.0)
    }

    /// 平均纯解码速度（tokens/s），无有效时长样本时返回 `None`。
    pub fn avg_speed_tps(&self) -> Option<f64> {
        if self.gen_ms == 0 {
            return None;
        }
        Some((self.speed_tokens as f64 / self.gen_ms as f64) * 1000.0)
    }

    /// 将单条已计价事件累加到本行。
    pub fn add_event(&mut self, event: &UsageEvent) {
        self.requests = self.requests.saturating_add(1);
        self.input = self.input.saturating_add(event.usage.input);
        self.output = self.output.saturating_add(event.usage.output);
        self.reasoning = self.reasoning.saturating_add(event.usage.reasoning);
        self.cache_read = self.cache_read.saturating_add(event.usage.cache_read);
        self.cache_write = self.cache_write.saturating_add(event.usage.cache_write);

        if let Some(usd) = event.cost_usd {
            self.cost_usd += usd;
        }

        if let Some(ttft) = event.ttft_ms {
            self.ttft_sum_ms = self.ttft_sum_ms.saturating_add(ttft);
            self.ttft_count = self.ttft_count.saturating_add(1);
        }

        if let Some(dur) = event.duration_ms {
            if dur > 0 {
                // 速度分子与分母严格同源：只有带时长的样本才计入 speed_tokens
                self.gen_ms = self.gen_ms.saturating_add(dur);
                self.speed_tokens = self
                    .speed_tokens
                    .saturating_add(event.usage.output_with_reasoning());
            }
        }
    }

    /// 将另一条聚合行折叠合并到本行。
    pub fn fold(&mut self, other: &BucketRow) {
        self.requests = self.requests.saturating_add(other.requests);
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.cost_usd += other.cost_usd;
        self.ttft_sum_ms = self.ttft_sum_ms.saturating_add(other.ttft_sum_ms);
        self.ttft_count = self.ttft_count.saturating_add(other.ttft_count);
        self.gen_ms = self.gen_ms.saturating_add(other.gen_ms);
        self.speed_tokens = self.speed_tokens.saturating_add(other.speed_tokens);
    }
}

/// 单个 `(agent, provider, model)` 分组，包含升序分桶行与末尾 `Total`。
#[derive(Debug, Clone)]
pub struct ModelGroup {
    pub agent: Agent,
    pub provider: String,
    pub model: String,
    pub pricing: ModelPricing,
    pub rows: Vec<BucketRow>,
    pub total: BucketRow,
}

/// 跨全部有价分组汇总出的总计表。
#[derive(Debug, Clone)]
pub struct Overall {
    pub rows: Vec<BucketRow>,
    pub total: BucketRow,
}

/// 完整聚合报表，供纯文本渲染与 TUI 共用。
#[derive(Debug, Clone)]
pub struct Report {
    pub granularity: Granularity,
    pub overall: Overall,
    pub groups: Vec<ModelGroup>,
    /// 本次查询中未匹配到价目表的 `agent/provider/model` 列表。
    pub unknown_models: Vec<String>,
}

/// 将归一化事件列表聚合为总计表、有价分组表与未知模型列表。
pub fn aggregate(events: &[UsageEvent], granularity: Granularity) -> Report {
    // 按 (agent_id, provider, model) 字典序分组，确保输出顺序稳定
    let mut grouped: BTreeMap<
        (&'static str, String, String),
        (Agent, ModelPricing, BTreeMap<String, BucketRow>),
    > = BTreeMap::new();
    let mut unknown_set: BTreeSet<String> = BTreeSet::new();

    for event in events {
        let provider = event
            .provider
            .as_deref()
            .unwrap_or("unknown")
            .to_string();

        let pricing = match event.pricing {
            Some(ref pricing) => pricing.clone(),
            None => {
                unknown_set.insert(format!(
                    "{}/{}/{}",
                    event.agent.id(),
                    provider,
                    event.model
                ));
                continue;
            }
        };

        let key = (event.agent.id(), provider, event.model.clone());
        let entry = grouped
            .entry(key)
            .or_insert_with(|| (event.agent, pricing, BTreeMap::new()));

        let bucket = bucket_key(event.ts_ms, granularity);
        let row = entry
            .2
            .entry(bucket.clone())
            .or_insert_with(|| BucketRow::empty(bucket));
        row.add_event(event);
    }

    let mut groups = Vec::with_capacity(grouped.len());
    let mut overall_by_bucket: BTreeMap<String, BucketRow> = BTreeMap::new();

    for ((_agent_id, provider, model), (agent, pricing, bucket_map)) in grouped {
        let mut rows = Vec::with_capacity(bucket_map.len());
        let mut total = BucketRow::empty("Total");

        for (bucket_label, row) in bucket_map {
            total.fold(&row);

            let overall_row = overall_by_bucket
                .entry(bucket_label.clone())
                .or_insert_with(|| BucketRow::empty(bucket_label));
            overall_row.fold(&row);

            rows.push(row);
        }

        groups.push(ModelGroup {
            agent,
            provider,
            model,
            pricing,
            rows,
            total,
        });
    }

    let mut overall_rows = Vec::with_capacity(overall_by_bucket.len());
    let mut overall_total = BucketRow::empty("Total");
    for (_label, row) in overall_by_bucket {
        overall_total.fold(&row);
        overall_rows.push(row);
    }

    Report {
        granularity,
        overall: Overall {
            rows: overall_rows,
            total: overall_total,
        },
        groups,
        unknown_models: unknown_set.into_iter().collect(),
    }
}
