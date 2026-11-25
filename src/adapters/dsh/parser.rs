//! dsh 单会话纯状态机层（无 I/O、无 JSON）。
//!
//! 消费 `DshEvent` 流，维护路由、step 起点、fork 继承前缀跳过、会话标题回填与 turn 中止回填。

use std::collections::{HashMap, HashSet};

use super::decode::DshEvent;
use crate::core::format::{folder_name_from_path, non_empty_trimmed};
use crate::core::model::{Agent, Usage, UsageEvent};
use crate::core::pricing::{calc_cost_usd, PricingCatalog};
use crate::core::time::TimeRange;

/// 消费 `DshEvent` 流并产出归一化 `UsageEvent` 的纯状态机。
pub(super) struct SessionParser {
    session_id: String,
    session_name: Option<String>,
    folder: Option<String>,
    provider: Option<String>,
    model: String,
    seed_length: i64,
    seen_seqs: HashSet<i64>,
    step_starts: HashMap<(i64, i64), i64>,
    open_turns: HashMap<i64, Vec<usize>>,
    events: Vec<UsageEvent>,
}

impl SessionParser {
    pub fn new(fallback_session: String) -> Self {
        Self {
            session_id: fallback_session,
            session_name: None,
            folder: None,
            provider: None,
            model: "unknown".to_string(),
            seed_length: 0,
            seen_seqs: HashSet::new(),
            step_starts: HashMap::new(),
            open_turns: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// 推进单条语义事件。
    pub fn apply(&mut self, event: DshEvent, range: TimeRange, catalog: &PricingCatalog) {
        match event {
            DshEvent::Session {
                id,
                cwd,
                seed_length,
            } => {
                if let Some(id) = id {
                    self.session_id = id;
                }
                if let Some(cwd) = cwd {
                    self.folder = folder_name_from_path(&cwd);
                }
                if seed_length > 0 {
                    self.seed_length = seed_length;
                }
            }
            DshEvent::SessionTitle { title } => {
                if let Some(name) = title.as_deref().and_then(non_empty_trimmed) {
                    self.session_name = Some(name);
                }
            }
            DshEvent::Route { provider, model } => {
                if let Some(p) = provider {
                    self.provider = Some(p);
                }
                if let Some(m) = model {
                    self.model = m;
                }
            }
            DshEvent::StepStart { key, ts_ms } => {
                self.step_starts.insert(key, ts_ms);
            }
            DshEvent::TurnEnd { turn, aborted } => {
                if let Some(indices) = self.open_turns.remove(&turn) {
                    if aborted {
                        for idx in indices {
                            if let Some(ev) = self.events.get_mut(idx) {
                                ev.aborted = true;
                            }
                        }
                    }
                }
            }
            DshEvent::AssistantMessage {
                seq,
                ts_ms,
                turn,
                step,
                usage,
                stream_times,
            } => {
                self.record_message(seq, ts_ms, turn, step, usage, &stream_times, range, catalog);
            }
        }
    }

    /// 校验去重、时间窗与速度样本，记录一条 `UsageEvent`。
    #[allow(clippy::too_many_arguments)]
    fn record_message(
        &mut self,
        seq: Option<i64>,
        ts_ms: i64,
        turn: Option<i64>,
        step: Option<i64>,
        usage: Usage,
        stream_times: &[i64],
        range: TimeRange,
        catalog: &PricingCatalog,
    ) {
        if let Some(seq) = seq {
            if self.seed_length > 0 && seq <= self.seed_length {
                return;
            }
            if !self.seen_seqs.insert(seq) {
                return;
            }
        }

        if ts_ms < range.start_ms || ts_ms >= range.end_ms || usage.total() == 0 {
            return;
        }

        let identity = catalog.identify(&self.model);
        let cost_usd = identity
            .pricing
            .as_ref()
            .map(|pricing| calc_cost_usd(pricing, &usage));

        let step_key = match (turn, step) {
            (Some(t), Some(s)) => Some((t, s)),
            _ => None,
        };
        let (ttft_ms, duration_ms) = compute_stream_timing(stream_times, step_key, &self.step_starts);

        let idx = self.events.len();
        self.events.push(UsageEvent {
            agent: Agent::Dsh,
            session: self.session_id.clone(),
            session_name: self.session_name.clone(),
            folder: self.folder.clone(),
            provider: self.provider.clone(),
            model: identity.name,
            ts_ms,
            usage,
            cost_usd,
            pricing: identity.pricing,
            ttft_ms,
            duration_ms,
            role: None,
            turn: turn.map(|t| t.to_string()),
            aborted: false,
        });

        if let Some(turn_num) = turn {
            self.open_turns.entry(turn_num).or_default().push(idx);
        }
    }

    pub fn into_events(mut self) -> Vec<UsageEvent> {
        // `session/title` 常在首轮助手消息之后产出，统一回填到整场会话事件
        if let Some(ref name) = self.session_name {
            for ev in &mut self.events {
                ev.session_name = Some(name.clone());
            }
        }
        self.events
    }
}

/// 严格基于 `stream[]` 分块时间戳计算 TTFT 与纯解码窗口。
fn compute_stream_timing(
    stream_times: &[i64],
    step_key: Option<(i64, i64)>,
    step_starts: &HashMap<(i64, i64), i64>,
) -> (Option<u64>, Option<u64>) {
    if stream_times.is_empty() {
        return (None, None);
    }

    let mut first = stream_times[0];
    let mut last = stream_times[0];
    for &t in &stream_times[1..] {
        if t < first {
            first = t;
        }
        if t > last {
            last = t;
        }
    }

    let mut ttft_ms = None;
    if let Some(key) = step_key {
        if let Some(&step_start) = step_starts.get(&key) {
            if first >= step_start {
                ttft_ms = Some((first - step_start) as u64);
            }
        }
    }

    let mut duration_ms = None;
    if stream_times.len() >= 2 && last > first {
        duration_ms = Some((last - first) as u64);
    }

    (ttft_ms, duration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_parser_computes_speed_and_backfills_aborted() {
        let catalog = PricingCatalog::load_default().unwrap();
        let mut parser = SessionParser::new("fallback".to_string());

        let stream = vec![
            DshEvent::Session {
                id: Some("session-test".to_string()),
                cwd: Some("/Users/me/Code/demo".to_string()),
                seed_length: 0,
            },
            DshEvent::Route {
                provider: Some("deepseek-official".to_string()),
                model: Some("deepseek-flash".to_string()),
            },
            DshEvent::StepStart {
                key: (1, 1),
                ts_ms: 1_000,
            },
            DshEvent::AssistantMessage {
                seq: Some(10),
                ts_ms: 5_000,
                turn: Some(1),
                step: Some(1),
                usage: Usage {
                    input: 100,
                    output: 200,
                    reasoning: 0,
                    cache_read: 800,
                    cache_write: 0,
                },
                stream_times: vec![2_500, 4_500],
            },
            DshEvent::TurnEnd {
                turn: 1,
                aborted: true,
            },
            DshEvent::SessionTitle {
                title: Some("demo title".to_string()),
            },
        ];

        for ev in stream {
            parser.apply(ev, TimeRange::ALL, &catalog);
        }

        let events = parser.into_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session, "session-test");
        assert_eq!(events[0].session_name.as_deref(), Some("demo title"));
        assert_eq!(events[0].folder.as_deref(), Some("demo"));
        assert_eq!(events[0].ttft_ms, Some(1_500));
        assert_eq!(events[0].duration_ms, Some(2_000));
        assert!(events[0].aborted);
    }
}
