//! 会话（Session）与轮次（Turn）聚合逻辑（CLI 与 TUI 共用）。
//!
//! 两层粒度（不展开到碎步骤）：
//! - 会话列表：按最后活跃时间倒序展示完整对话；
//! - 单会话详情：按 ID 前缀匹配唯一会话，或一次性按 Agent 聚合全量会话与按时间倒序排列（最新在最前）的 Turn 列表。

mod render;

pub use render::{render_session_detail, render_session_list};

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Result};

use crate::core::format::short_id;
use crate::core::model::{Agent, UsageEvent};

/// 单个会话的聚合摘要。
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub agent: Agent,
    pub session_id: String,
    pub label: Option<String>,
    pub first_ts_ms: i64,
    pub last_ts_ms: i64,
    /// 按总 token 降序排列的模型列表（首个为主模型）。
    pub models: Vec<String>,
    pub turns: usize,
    pub steps: usize,
    pub aborted_steps: usize,
    /// 单步最大输入上下文（`input + cache_read + cache_write`）。
    pub peak_ctx: u64,
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost_usd: f64,
    pub ttft_sum_ms: u64,
    pub ttft_count: u64,
    pub gen_ms: u64,
    pub speed_tokens: u64,
}

/// 单个对话轮次（一次用户指令 -> Agent 跑多步完成）的聚合摘要。
#[derive(Debug, Clone)]
pub struct TurnSummary {
    /// 会话内按时间排序的 1-based 轮次编号。
    pub index: usize,
    pub turn_id: String,
    pub first_ts_ms: i64,
    pub last_ts_ms: i64,
    /// 按总 token 降序排列的模型列表。
    pub models: Vec<String>,
    pub steps: usize,
    pub aborted_steps: usize,
    pub peak_ctx: u64,
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost_usd: f64,
    pub ttft_sum_ms: u64,
    pub ttft_count: u64,
    pub gen_ms: u64,
    pub speed_tokens: u64,
}

/// 单个会话的完整视图：总览摘要 + 按时间倒序排列（最新在最前）的 Turn 列表。
#[derive(Debug, Clone)]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub turns: Vec<TurnSummary>,
}

/// 内部通用累加器，供会话和轮次两层共用。
struct MetricsAcc {
    first_ts_ms: i64,
    last_ts_ms: i64,
    model_tokens: HashMap<String, u64>,
    steps: usize,
    aborted_steps: usize,
    peak_ctx: u64,
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
    cost_usd: f64,
    ttft_sum_ms: u64,
    ttft_count: u64,
    gen_ms: u64,
    speed_tokens: u64,
}

impl MetricsAcc {
    fn new(ts_ms: i64) -> Self {
        Self {
            first_ts_ms: ts_ms,
            last_ts_ms: ts_ms,
            model_tokens: HashMap::new(),
            steps: 0,
            aborted_steps: 0,
            peak_ctx: 0,
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

    /// 累加单条请求事件。
    fn add(&mut self, event: &UsageEvent) {
        if event.ts_ms < self.first_ts_ms {
            self.first_ts_ms = event.ts_ms;
        }
        if event.ts_ms > self.last_ts_ms {
            self.last_ts_ms = event.ts_ms;
        }

        self.steps += 1;
        if event.aborted {
            self.aborted_steps += 1;
        }

        let ctx = event
            .usage
            .input
            .saturating_add(event.usage.cache_read)
            .saturating_add(event.usage.cache_write);
        if ctx > self.peak_ctx {
            self.peak_ctx = ctx;
        }

        self.input = self.input.saturating_add(event.usage.input);
        self.output = self.output.saturating_add(event.usage.output);
        self.reasoning = self.reasoning.saturating_add(event.usage.reasoning);
        self.cache_read = self.cache_read.saturating_add(event.usage.cache_read);
        self.cache_write = self.cache_write.saturating_add(event.usage.cache_write);

        let entry = self.model_tokens.entry(event.model.clone()).or_insert(0);
        *entry = entry.saturating_add(event.usage.total());

        if let Some(usd) = event.cost_usd {
            self.cost_usd += usd;
        }

        if let Some(ttft) = event.ttft_ms {
            self.ttft_sum_ms = self.ttft_sum_ms.saturating_add(ttft);
            self.ttft_count = self.ttft_count.saturating_add(1);
        }

        if let Some(dur) = event.duration_ms {
            if dur > 0 {
                self.gen_ms = self.gen_ms.saturating_add(dur);
                self.speed_tokens = self
                    .speed_tokens
                    .saturating_add(event.usage.output_with_reasoning());
            }
        }
    }

    /// 输出按 token 总量降序排列的模型名列表。
    fn sorted_models(&self) -> Vec<String> {
        let mut pairs: Vec<(&String, &u64)> = self.model_tokens.iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        pairs.into_iter().map(|(m, _)| m.clone()).collect()
    }

    /// 转换为 `SessionSummary`（避免多处重复逐字段拷贝）。
    fn into_session_summary(
        self,
        agent: Agent,
        session_id: String,
        label: Option<String>,
        turns: usize,
    ) -> SessionSummary {
        let models = self.sorted_models();
        SessionSummary {
            agent,
            session_id,
            label,
            first_ts_ms: self.first_ts_ms,
            last_ts_ms: self.last_ts_ms,
            models,
            turns,
            steps: self.steps,
            aborted_steps: self.aborted_steps,
            peak_ctx: self.peak_ctx,
            input: self.input,
            output: self.output,
            reasoning: self.reasoning,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            cost_usd: self.cost_usd,
            ttft_sum_ms: self.ttft_sum_ms,
            ttft_count: self.ttft_count,
            gen_ms: self.gen_ms,
            speed_tokens: self.speed_tokens,
        }
    }

    /// 转换为 `TurnSummary`。
    fn into_turn_summary(self, index: usize, turn_id: String) -> TurnSummary {
        let models = self.sorted_models();
        TurnSummary {
            index,
            turn_id,
            first_ts_ms: self.first_ts_ms,
            last_ts_ms: self.last_ts_ms,
            models,
            steps: self.steps,
            aborted_steps: self.aborted_steps,
            peak_ctx: self.peak_ctx,
            input: self.input,
            output: self.output,
            reasoning: self.reasoning,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            cost_usd: self.cost_usd,
            ttft_sum_ms: self.ttft_sum_ms,
            ttft_count: self.ttft_count,
            gen_ms: self.gen_ms,
            speed_tokens: self.speed_tokens,
        }
    }
}

/// 将 Turn 映射表先按首次出现时间升序分配 1-based 编号（`#1..#N`），再倒序返回（最新轮次排最前）。
fn finalize_turns(by_turn: HashMap<String, MetricsAcc>) -> Vec<TurnSummary> {
    let mut entries: Vec<(String, MetricsAcc)> = by_turn.into_iter().collect();
    entries.sort_by(|a, b| {
        a.1.first_ts_ms
            .cmp(&b.1.first_ts_ms)
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut turns: Vec<TurnSummary> = entries
        .into_iter()
        .enumerate()
        .map(|(idx, (turn_id, acc))| acc.into_turn_summary(idx + 1, turn_id))
        .collect();
    turns.reverse();
    turns
}

impl SessionSummary {
    pub fn total_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub fn cache_percent(&self) -> Option<f64> {
        let total = self.total_tokens();
        if total == 0 {
            return None;
        }
        let cached = self.cache_read.saturating_add(self.cache_write);
        Some((cached as f64 / total as f64) * 100.0)
    }

    pub fn avg_ttft_secs(&self) -> Option<f64> {
        if self.ttft_count == 0 {
            return None;
        }
        Some((self.ttft_sum_ms as f64 / self.ttft_count as f64) / 1000.0)
    }

    pub fn avg_speed_tps(&self) -> Option<f64> {
        if self.gen_ms == 0 {
            return None;
        }
        Some((self.speed_tokens as f64 / self.gen_ms as f64) * 1000.0)
    }

    pub fn span_secs(&self) -> u64 {
        if self.last_ts_ms > self.first_ts_ms {
            ((self.last_ts_ms - self.first_ts_ms) / 1000) as u64
        } else {
            0
        }
    }
}

impl TurnSummary {
    pub fn total_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub fn cache_percent(&self) -> Option<f64> {
        let total = self.total_tokens();
        if total == 0 {
            return None;
        }
        let cached = self.cache_read.saturating_add(self.cache_write);
        Some((cached as f64 / total as f64) * 100.0)
    }

    pub fn avg_ttft_secs(&self) -> Option<f64> {
        if self.ttft_count == 0 {
            return None;
        }
        Some((self.ttft_sum_ms as f64 / self.ttft_count as f64) / 1000.0)
    }

    pub fn avg_speed_tps(&self) -> Option<f64> {
        if self.gen_ms == 0 {
            return None;
        }
        Some((self.speed_tokens as f64 / self.gen_ms as f64) * 1000.0)
    }

    pub fn span_secs(&self) -> u64 {
        if self.last_ts_ms > self.first_ts_ms {
            ((self.last_ts_ms - self.first_ts_ms) / 1000) as u64
        } else {
            0
        }
    }
}

/// 将事件流按 `(agent, session)` 聚合，并按最后活跃时间降序返回。
pub fn summarize_sessions(events: &[UsageEvent]) -> Vec<SessionSummary> {
    struct SessionState {
        agent: Agent,
        label: Option<String>,
        turns: HashSet<String>,
        acc: MetricsAcc,
    }

    let mut by_session: BTreeMap<(&'static str, String), SessionState> = BTreeMap::new();

    for event in events {
        let key = (event.agent.id(), event.session.clone());
        let state = by_session.entry(key).or_insert_with(|| SessionState {
            agent: event.agent,
            label: event.session_label.clone(),
            turns: HashSet::new(),
            acc: MetricsAcc::new(event.ts_ms),
        });

        if state.label.is_none() && event.session_label.is_some() {
            state.label = event.session_label.clone();
        }
        let turn_key = event.turn.as_deref().unwrap_or("default");
        state.turns.insert(turn_key.to_string());
        state.acc.add(event);
    }

    let mut summaries = Vec::with_capacity(by_session.len());
    for ((_, session_id), state) in by_session {
        summaries.push(state.acc.into_session_summary(
            state.agent,
            session_id,
            state.label,
            state.turns.len(),
        ));
    }

    // 最近活跃的会话排在最前
    summaries.sort_by(|a, b| {
        b.last_ts_ms
            .cmp(&a.last_ts_ms)
            .then_with(|| a.agent.id().cmp(b.agent.id()))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    summaries
}

/// 一次性将事件流中指定 Agent 的事件聚合为完整会话及 Turn 明细。
///
/// 返回的会话按最后活跃时间倒序排列，每个会话内的 Turn 也按时间倒序排列（最新在最前，保留 `#1..#N` 原编号）。
pub fn summarize_sessions_with_turns(events: &[UsageEvent], agent: Agent) -> Vec<SessionDetail> {
    struct SessionTurnsState {
        label: Option<String>,
        session_acc: MetricsAcc,
        by_turn: HashMap<String, MetricsAcc>,
    }

    let mut by_session: HashMap<String, SessionTurnsState> = HashMap::new();

    for event in events {
        if event.agent != agent {
            continue;
        }

        let state = by_session
            .entry(event.session.clone())
            .or_insert_with(|| SessionTurnsState {
                label: event.session_label.clone(),
                session_acc: MetricsAcc::new(event.ts_ms),
                by_turn: HashMap::new(),
            });

        if state.label.is_none() && event.session_label.is_some() {
            state.label = event.session_label.clone();
        }
        state.session_acc.add(event);

        let turn_key = event.turn.as_deref().unwrap_or("default").to_string();
        let turn_acc = state
            .by_turn
            .entry(turn_key)
            .or_insert_with(|| MetricsAcc::new(event.ts_ms));
        turn_acc.add(event);
    }

    let mut details = Vec::with_capacity(by_session.len());
    for (session_id, state) in by_session {
        let turns = finalize_turns(state.by_turn);
        let summary =
            state
                .session_acc
                .into_session_summary(agent, session_id, state.label, turns.len());
        details.push(SessionDetail { summary, turns });
    }

    details.sort_by(|a, b| {
        b.summary
            .last_ts_ms
            .cmp(&a.summary.last_ts_ms)
            .then_with(|| a.summary.session_id.cmp(&b.summary.session_id))
    });

    details
}

/// 按精确 ID 或唯一前缀查找单个会话（兼容跨 Agent 查询），并按 Turn 粒度聚合返回详情。
pub fn find_session_detail(events: &[UsageEvent], id_prefix: &str) -> Result<SessionDetail> {
    let query = id_prefix.trim();
    if query.is_empty() {
        bail!("session id prefix must not be empty");
    }

    let mut all_keys: HashSet<(Agent, &str)> = HashSet::new();
    for event in events {
        all_keys.insert((event.agent, event.session.as_str()));
    }

    // 优先尝试精确匹配
    let mut exact: Vec<(Agent, &str)> = all_keys
        .iter()
        .copied()
        .filter(|(_, id)| *id == query)
        .collect();
    exact.sort_by(|a, b| a.0.id().cmp(b.0.id()).then_with(|| a.1.cmp(b.1)));

    let (matched_agent, matched_id): (Agent, String) = if exact.len() == 1 {
        (exact[0].0, exact[0].1.to_string())
    } else if exact.len() > 1 {
        let preview: Vec<String> = exact
            .iter()
            .take(5)
            .map(|(a, id)| format!("{}/{}", a.id(), short_id(id, 12)))
            .collect();
        bail!(
            "ambiguous session id '{}' matches {} agents: {} (specify -a <agent>)",
            query,
            exact.len(),
            preview.join(", ")
        );
    } else {
        let query_lower = query.to_ascii_lowercase();
        let mut matches: Vec<(Agent, &str)> = all_keys
            .iter()
            .copied()
            .filter(|(_, id)| id.to_ascii_lowercase().starts_with(&query_lower))
            .collect();
        matches.sort_by(|a, b| a.0.id().cmp(b.0.id()).then_with(|| a.1.cmp(b.1)));

        if matches.is_empty() {
            bail!("session '{}' not found", query);
        }
        if matches.len() > 1 {
            let preview: Vec<String> = matches
                .iter()
                .take(5)
                .map(|(a, id)| format!("{}/{}", a.id(), short_id(id, 12)))
                .collect();
            bail!(
                "ambiguous session prefix '{}' matches {} sessions: {}",
                query,
                matches.len(),
                preview.join(", ")
            );
        }
        (matches[0].0, matches[0].1.to_string())
    };

    let mut session_events: Vec<&UsageEvent> = Vec::new();
    for event in events {
        if event.agent == matched_agent && event.session == matched_id {
            session_events.push(event);
        }
    }

    if session_events.is_empty() {
        bail!("session '{}' has no usage events", matched_id);
    }

    let mut label: Option<String> = None;
    let mut session_acc = MetricsAcc::new(session_events[0].ts_ms);
    let mut by_turn: HashMap<String, MetricsAcc> = HashMap::new();

    for event in &session_events {
        if label.is_none() && event.session_label.is_some() {
            label = event.session_label.clone();
        }
        session_acc.add(event);

        let turn_key = event.turn.as_deref().unwrap_or("default").to_string();
        let turn_acc = by_turn
            .entry(turn_key)
            .or_insert_with(|| MetricsAcc::new(event.ts_ms));
        turn_acc.add(event);
    }

    let turns = finalize_turns(by_turn);
    let summary = session_acc.into_session_summary(matched_agent, matched_id, label, turns.len());

    Ok(SessionDetail { summary, turns })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Usage;

    fn make_event(session: &str, turn: &str, ts_ms: i64, input: u64, output: u64) -> UsageEvent {
        UsageEvent {
            agent: Agent::Dsh,
            session: session.to_string(),
            session_label: Some("demo".to_string()),
            provider: Some("deepseek-official".to_string()),
            model: "deepseek-v4.1-flash".to_string(),
            ts_ms,
            usage: Usage {
                input,
                output,
                reasoning: 0,
                cache_read: 10_000,
                cache_write: 0,
            },
            cost_usd: Some(0.01),
            pricing: None,
            ttft_ms: Some(1500),
            duration_ms: Some(1000),
            role: None,
            turn: Some(turn.to_string()),
            aborted: false,
        }
    }

    #[test]
    fn summarizes_sessions_and_resolves_unique_prefix() {
        let events = vec![
            make_event("abc11111-xxxx", "1", 1_700_000_000_000, 1000, 200),
            make_event("abc11111-xxxx", "1", 1_700_000_010_000, 2000, 300),
            make_event("abc11111-xxxx", "2", 1_700_000_060_000, 3000, 400),
            make_event("abc22222-yyyy", "1", 1_700_000_120_000, 500, 100),
        ];

        let list = summarize_sessions(&events);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].session_id, "abc22222-yyyy");
        assert_eq!(list[1].session_id, "abc11111-xxxx");
        assert_eq!(list[1].turns, 2);
        assert_eq!(list[1].steps, 3);
        assert_eq!(list[1].peak_ctx, 13_000);

        let detail = find_session_detail(&events, "abc1").unwrap();
        assert_eq!(detail.summary.session_id, "abc11111-xxxx");
        assert_eq!(detail.turns.len(), 2);
        assert_eq!(detail.turns[0].index, 2);
        assert_eq!(detail.turns[0].steps, 1);
        assert_eq!(detail.turns[1].index, 1);
        assert_eq!(detail.turns[1].steps, 2);

        assert!(find_session_detail(&events, "abc").is_err());
        assert!(find_session_detail(&events, "zzz").is_err());

        let dsh_details = summarize_sessions_with_turns(&events, Agent::Dsh);
        assert_eq!(dsh_details.len(), 2);
        assert_eq!(dsh_details[0].summary.session_id, "abc22222-yyyy");
        assert_eq!(dsh_details[1].summary.session_id, "abc11111-xxxx");
        assert_eq!(dsh_details[1].turns.len(), 2);
        assert_eq!(dsh_details[1].turns[0].index, 2);
        assert_eq!(dsh_details[1].turns[1].index, 1);

        let agy_details = summarize_sessions_with_turns(&events, Agent::Agy);
        assert!(agy_details.is_empty());
    }
}
