//! 会话（Session）与轮次（Turn）聚合及纯文本渲染。
//!
//! 两层粒度（不展开到碎步骤）：
//! - 会话列表：按最后活跃时间倒序展示最近 N 次完整对话；
//! - 单会话详情：按 ID 前缀匹配唯一会话，展示顶部摘要与按时间升序排列的 Turn 列表。

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Result};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::core::model::{Agent, UsageEvent};
use crate::core::time::format_datetime;

/// 单个会话的聚合摘要。
#[derive(Debug, Clone)]
pub struct SessionSummary {
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

/// 单个会话的完整视图：总览摘要 + 按时间升序排列的 Turn 列表。
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
        let mut models = Vec::with_capacity(pairs.len());
        for (model, _) in pairs {
            models.push(model.clone());
        }
        models
    }
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

/// 将事件流按会话聚合，并按最后活跃时间降序返回。
pub fn summarize_sessions(events: &[UsageEvent]) -> Vec<SessionSummary> {
    struct SessionState {
        label: Option<String>,
        turns: HashSet<String>,
        acc: MetricsAcc,
    }

    let mut by_session: BTreeMap<String, SessionState> = BTreeMap::new();

    for event in events {
        let state = by_session
            .entry(event.session.clone())
            .or_insert_with(|| SessionState {
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
    for (session_id, state) in by_session {
        let models = state.acc.sorted_models();
        summaries.push(SessionSummary {
            session_id,
            label: state.label,
            first_ts_ms: state.acc.first_ts_ms,
            last_ts_ms: state.acc.last_ts_ms,
            models,
            turns: state.turns.len(),
            steps: state.acc.steps,
            aborted_steps: state.acc.aborted_steps,
            peak_ctx: state.acc.peak_ctx,
            input: state.acc.input,
            output: state.acc.output,
            reasoning: state.acc.reasoning,
            cache_read: state.acc.cache_read,
            cache_write: state.acc.cache_write,
            cost_usd: state.acc.cost_usd,
            ttft_sum_ms: state.acc.ttft_sum_ms,
            ttft_count: state.acc.ttft_count,
            gen_ms: state.acc.gen_ms,
            speed_tokens: state.acc.speed_tokens,
        });
    }

    // 最近活跃的会话排在最前
    summaries.sort_by(|a, b| {
        b.last_ts_ms
            .cmp(&a.last_ts_ms)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    summaries
}

/// 一次性将事件流中指定 Agent 的事件聚合为完整会话及 Turn 明细。
///
/// 返回的会话按最后活跃时间倒序排列，每个会话内的 Turn 按首次出现时间升序排列。
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
        let mut turn_entries: Vec<(String, MetricsAcc)> = state.by_turn.into_iter().collect();
        turn_entries.sort_by(|a, b| {
            a.1.first_ts_ms
                .cmp(&b.1.first_ts_ms)
                .then_with(|| a.0.cmp(&b.0))
        });

        let mut turns = Vec::with_capacity(turn_entries.len());
        for (idx, (turn_id, acc)) in turn_entries.into_iter().enumerate() {
            let models = acc.sorted_models();
            turns.push(TurnSummary {
                index: idx + 1,
                turn_id,
                first_ts_ms: acc.first_ts_ms,
                last_ts_ms: acc.last_ts_ms,
                models,
                steps: acc.steps,
                aborted_steps: acc.aborted_steps,
                peak_ctx: acc.peak_ctx,
                input: acc.input,
                output: acc.output,
                reasoning: acc.reasoning,
                cache_read: acc.cache_read,
                cache_write: acc.cache_write,
                cost_usd: acc.cost_usd,
                ttft_sum_ms: acc.ttft_sum_ms,
                ttft_count: acc.ttft_count,
                gen_ms: acc.gen_ms,
                speed_tokens: acc.speed_tokens,
            });
        }

        let summary = SessionSummary {
            session_id,
            label: state.label,
            first_ts_ms: state.session_acc.first_ts_ms,
            last_ts_ms: state.session_acc.last_ts_ms,
            models: state.session_acc.sorted_models(),
            turns: turns.len(),
            steps: state.session_acc.steps,
            aborted_steps: state.session_acc.aborted_steps,
            peak_ctx: state.session_acc.peak_ctx,
            input: state.session_acc.input,
            output: state.session_acc.output,
            reasoning: state.session_acc.reasoning,
            cache_read: state.session_acc.cache_read,
            cache_write: state.session_acc.cache_write,
            cost_usd: state.session_acc.cost_usd,
            ttft_sum_ms: state.session_acc.ttft_sum_ms,
            ttft_count: state.session_acc.ttft_count,
            gen_ms: state.session_acc.gen_ms,
            speed_tokens: state.session_acc.speed_tokens,
        };

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

/// 按精确 ID 或唯一前缀查找单个会话，并按 Turn 粒度聚合返回详情。
pub fn find_session_detail(events: &[UsageEvent], id_prefix: &str) -> Result<SessionDetail> {
    let query = id_prefix.trim();
    if query.is_empty() {
        bail!("session id prefix must not be empty");
    }

    let mut all_ids: HashSet<&str> = HashSet::new();
    for event in events {
        all_ids.insert(event.session.as_str());
    }

    // 优先尝试精确匹配
    let matched_id: String = if all_ids.contains(query) {
        query.to_string()
    } else {
        let query_lower = query.to_ascii_lowercase();
        let mut matches: Vec<&str> = Vec::new();
        for id in &all_ids {
            if id.to_ascii_lowercase().starts_with(&query_lower) {
                matches.push(id);
            }
        }
        matches.sort_unstable();

        if matches.is_empty() {
            bail!("session '{}' not found", query);
        }
        if matches.len() > 1 {
            let mut preview = Vec::new();
            for id in matches.iter().take(5) {
                preview.push(short_id(id));
            }
            bail!(
                "ambiguous session prefix '{}' matches {} sessions: {}",
                query,
                matches.len(),
                preview.join(", ")
            );
        }
        matches[0].to_string()
    };

    let mut session_events: Vec<&UsageEvent> = Vec::new();
    for event in events {
        if event.session == matched_id {
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

    // 按各 Turn 首次出现时间升序排列，并赋予 1-based 序号
    let mut turn_entries: Vec<(String, MetricsAcc)> = by_turn.into_iter().collect();
    turn_entries.sort_by(|a, b| {
        a.1.first_ts_ms
            .cmp(&b.1.first_ts_ms)
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut turns = Vec::with_capacity(turn_entries.len());
    for (idx, (turn_id, acc)) in turn_entries.into_iter().enumerate() {
        let models = acc.sorted_models();
        turns.push(TurnSummary {
            index: idx + 1,
            turn_id,
            first_ts_ms: acc.first_ts_ms,
            last_ts_ms: acc.last_ts_ms,
            models,
            steps: acc.steps,
            aborted_steps: acc.aborted_steps,
            peak_ctx: acc.peak_ctx,
            input: acc.input,
            output: acc.output,
            reasoning: acc.reasoning,
            cache_read: acc.cache_read,
            cache_write: acc.cache_write,
            cost_usd: acc.cost_usd,
            ttft_sum_ms: acc.ttft_sum_ms,
            ttft_count: acc.ttft_count,
            gen_ms: acc.gen_ms,
            speed_tokens: acc.speed_tokens,
        });
    }

    let summary = SessionSummary {
        session_id: matched_id,
        label,
        first_ts_ms: session_acc.first_ts_ms,
        last_ts_ms: session_acc.last_ts_ms,
        models: session_acc.sorted_models(),
        turns: turns.len(),
        steps: session_acc.steps,
        aborted_steps: session_acc.aborted_steps,
        peak_ctx: session_acc.peak_ctx,
        input: session_acc.input,
        output: session_acc.output,
        reasoning: session_acc.reasoning,
        cache_read: session_acc.cache_read,
        cache_write: session_acc.cache_write,
        cost_usd: session_acc.cost_usd,
        ttft_sum_ms: session_acc.ttft_sum_ms,
        ttft_count: session_acc.ttft_count,
        gen_ms: session_acc.gen_ms,
        speed_tokens: session_acc.speed_tokens,
    };

    Ok(SessionDetail { summary, turns })
}

/// 渲染纯文本对齐的会话列表（最多展示 `limit` 条）。
pub fn render_session_list(sessions: &[SessionSummary], limit: usize) -> String {
    if sessions.is_empty() {
        return "no sessions found".to_string();
    }

    let slice_len = sessions.len().min(limit);
    let shown = &sessions[..slice_len];

    let headers = [
        "Time", "Session", "Label", "Model", "Turns", "Steps", "PeakCtx", "Tokens", "Cache%",
        "Cost", "TTFT", "Speed",
    ];
    let right_align = [
        false, false, false, false, true, true, true, true, true, true, true, true,
    ];

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(shown.len());
    for s in shown {
        let label = match s.label.as_deref() {
            Some(text) => truncate_chars(text, 20),
            None => "—".to_string(),
        };
        rows.push(vec![
            format_datetime(s.last_ts_ms),
            short_id(&s.session_id),
            label,
            format_models_cell(&s.models),
            s.turns.to_string(),
            s.steps.to_string(),
            format_compact_tokens(s.peak_ctx),
            format_int(s.total_tokens()),
            format_percent(s.cache_percent()),
            format!("${:.3}", s.cost_usd),
            format_ttft(s.avg_ttft_secs()),
            format_speed(s.avg_speed_tps()),
        ]);
    }

    render_aligned_table(&headers, &right_align, &rows)
}

/// 渲染单个会话的纯文本总览 + Turn 轮次明细表。
pub fn render_session_detail(detail: &SessionDetail) -> String {
    let s = &detail.summary;
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("session  {}", s.session_id));
    if let Some(ref label) = s.label {
        lines.push(format!("label    {}", label));
    }
    lines.push(format!(
        "window   {} .. {} ({})",
        format_datetime(s.first_ts_ms),
        format_datetime(s.last_ts_ms),
        format_span(s.span_secs()),
    ));
    lines.push(format!("models   {}", s.models.join(", ")));

    let mut totals = format!(
        "totals   turns {} / steps {} / peak ctx {} / tokens {} / cache {} / cost ${:.3} / ttft {} / speed {}",
        s.turns,
        s.steps,
        format_compact_tokens(s.peak_ctx),
        format_int(s.total_tokens()),
        format_percent(s.cache_percent()),
        s.cost_usd,
        format_ttft(s.avg_ttft_secs()),
        format_speed(s.avg_speed_tps()),
    );
    if s.aborted_steps > 0 {
        totals.push_str(&format!(" / aborted {}", s.aborted_steps));
    }
    lines.push(totals);
    lines.push(String::new());

    let headers = [
        "Turn", "Time", "Model", "Steps", "PeakCtx", "Tokens", "Cache%", "Cost", "TTFT", "Speed",
        "Span",
    ];
    let right_align = [
        true, false, false, true, true, true, true, true, true, true, true,
    ];

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(detail.turns.len() + 1);
    for t in &detail.turns {
        let turn_label = if t.aborted_steps > 0 {
            format!("#{}*", t.index)
        } else {
            format!("#{}", t.index)
        };
        rows.push(vec![
            turn_label,
            format_datetime(t.first_ts_ms),
            format_models_cell(&t.models),
            t.steps.to_string(),
            format_compact_tokens(t.peak_ctx),
            format_int(t.total_tokens()),
            format_percent(t.cache_percent()),
            format!("${:.3}", t.cost_usd),
            format_ttft(t.avg_ttft_secs()),
            format_speed(t.avg_speed_tps()),
            format_span(t.span_secs()),
        ]);
    }

    rows.push(vec![
        "Total".to_string(),
        "—".to_string(),
        "—".to_string(),
        s.steps.to_string(),
        format_compact_tokens(s.peak_ctx),
        format_int(s.total_tokens()),
        format_percent(s.cache_percent()),
        format!("${:.3}", s.cost_usd),
        format_ttft(s.avg_ttft_secs()),
        format_speed(s.avg_speed_tps()),
        format_span(s.span_secs()),
    ]);

    lines.push(render_aligned_table(&headers, &right_align, &rows));
    lines.join("\n")
}

/// 渲染通用纯空格对齐表格（无装饰线、无颜色，按终端显示列宽对齐以兼容中文标签）。
fn render_aligned_table(headers: &[&str], right_align: &[bool], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths = vec![0usize; cols];

    for i in 0..cols {
        widths[i] = UnicodeWidthStr::width(headers[i]);
    }
    for row in rows {
        for i in 0..cols {
            let w = UnicodeWidthStr::width(row[i].as_str());
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let mut out = Vec::with_capacity(rows.len() + 1);
    let header_cells: Vec<String> = headers.iter().map(|h| (*h).to_string()).collect();
    out.push(join_cells(&header_cells, &widths, right_align));
    for row in rows {
        out.push(join_cells(row, &widths, right_align));
    }
    out.join("\n")
}

/// 按显示列宽与对齐方向拼接单行。
fn join_cells(cells: &[String], widths: &[usize], right_align: &[bool]) -> String {
    let mut parts = Vec::with_capacity(cells.len());
    for i in 0..cells.len() {
        let len = UnicodeWidthStr::width(cells[i].as_str());
        let pad = widths[i].saturating_sub(len);
        let spaces = " ".repeat(pad);
        if right_align[i] {
            parts.push(format!("{}{}", spaces, cells[i]));
        } else {
            parts.push(format!("{}{}", cells[i], spaces));
        }
    }
    parts.join("  ").trim_end().to_string()
}

/// 截取会话 ID 前 12 个字符便于列表展示与 `-i` 复制。
fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 12 {
        id.to_string()
    } else {
        chars[..12].iter().collect()
    }
}

/// 按终端显示列宽截断过长标签并补省略号，防止中英文混排撑破列宽。
fn truncate_chars(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// 格式化模型列：单模型直接显示，多模型显示 `主模型 +N`。
fn format_models_cell(models: &[String]) -> String {
    match models.len() {
        0 => "unknown".to_string(),
        1 => truncate_chars(&models[0], 24),
        n => format!("{} +{}", truncate_chars(&models[0], 20), n - 1),
    }
}

/// 格式化紧凑 token 数量（如 `162k`、`1.2M`），用于 `PeakCtx` 列。
fn format_compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", (tokens as f64 / 1_000.0).round() as u64)
    } else {
        tokens.to_string()
    }
}

/// 将无符号整数格式化为千分位逗号形式。
fn format_int(n: u64) -> String {
    let raw = n.to_string();
    let len = raw.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, ch) in raw.chars().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn format_percent(val: Option<f64>) -> String {
    match val {
        Some(v) => format!("{:.1}%", v),
        None => "—".to_string(),
    }
}

fn format_ttft(val: Option<f64>) -> String {
    match val {
        Some(v) => format!("{:.1}s", v),
        None => "—".to_string(),
    }
}

fn format_speed(val: Option<f64>) -> String {
    match val {
        Some(v) => format!("{}/s", v.round() as u64),
        None => "—".to_string(),
    }
}

/// 格式化时间跨度（如 `42s`、`14m`、`2h15m`）。
fn format_span(secs: u64) -> String {
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    let rem_mins = mins % 60;
    if rem_mins == 0 {
        format!("{}h", hours)
    } else {
        format!("{}h{}m", hours, rem_mins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Agent, Usage};

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
        // 最近活跃的排在最前
        assert_eq!(list[0].session_id, "abc22222-yyyy");
        assert_eq!(list[1].session_id, "abc11111-xxxx");
        assert_eq!(list[1].turns, 2);
        assert_eq!(list[1].steps, 3);
        assert_eq!(list[1].peak_ctx, 13_000);

        // 唯一前缀匹配成功并按 Turn 聚合
        let detail = find_session_detail(&events, "abc1").unwrap();
        assert_eq!(detail.summary.session_id, "abc11111-xxxx");
        assert_eq!(detail.turns.len(), 2);
        assert_eq!(detail.turns[0].steps, 2);
        assert_eq!(detail.turns[1].steps, 1);

        // 歧义前缀与不存在前缀均报错
        assert!(find_session_detail(&events, "abc").is_err());
        assert!(find_session_detail(&events, "zzz").is_err());

        // 按 Agent 一次性聚合全部会话与 Turn 明细
        let dsh_details = summarize_sessions_with_turns(&events, Agent::Dsh);
        assert_eq!(dsh_details.len(), 2);
        assert_eq!(dsh_details[0].summary.session_id, "abc22222-yyyy");
        assert_eq!(dsh_details[1].summary.session_id, "abc11111-xxxx");
        assert_eq!(dsh_details[1].turns.len(), 2);
        assert_eq!(dsh_details[1].turns[0].index, 1);
        assert_eq!(dsh_details[1].turns[1].index, 2);

        let agy_details = summarize_sessions_with_turns(&events, Agent::Agy);
        assert!(agy_details.is_empty());
    }
}
