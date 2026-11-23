//! DeepSeek Harness（dsh）v4 zstd JSONL adapter。
//!
//! 扫描 `$DSH_HOME/sessions`（缺省 `~/.dsh/sessions`）下的 `*.v4.jsonl.zstd` 事件流，
//! 通过 `zstd` crate 在进程内流式解压（无需外部 `zstd` 命令）。
//!
//! 关键规则：
//! - 只解析 v4 格式；旧格式与 v3 静默跳过。
//! - 用量只从 `assistant/message` 的 `data.usage` 读取，不读 `stream[].chunk.usage` 以免双计。
//! - 速度指标严格取自 `stream[]` 分块时间戳：
//!   - `ttft_ms`：首块时间戳减去对应 `step/start` 时刻；
//!   - `duration_ms`：末块时间戳减去首块时间戳（纯解码窗口）。
//!   缺分块时间戳时如实留空 `None`，不用整步耗时伪造 TTFT 或解码速度。
//! - 若会话由 fork 产生且带有 `seedLength`，跳过 `seq <= seedLength` 的继承事件以免双计。

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use crate::model::{Agent, Usage, UsageEvent};
use crate::pricing::{calc_cost_usd, PricingCatalog};
use crate::time::TimeRange;

/// dsh v4 JSONL 单行事件信封。
#[derive(Debug, Deserialize)]
struct RawEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    seq: Option<i64>,
    time: Option<i64>,
    id: Option<String>,
    #[serde(rename = "seedLength")]
    seed_length: Option<i64>,
    data: Option<Value>,
}

/// 逐行解析单个会话文件时的上下文状态。
struct FileState {
    session_id: String,
    provider: Option<String>,
    model: String,
    /// fork 会话继承自父会话的前缀长度（普通会话为 0）。
    seed_length: i64,
    /// 文件内已处理过的事件序号，用于去重。
    seen_seqs: HashSet<i64>,
    /// 每个 `(turn, step)` 对应的 `step/start` 时刻。
    step_starts: HashMap<(i64, i64), i64>,
    /// 每个活跃 turn 对应在 `events` 中的索引，用于在 `turn/end` 回填 `aborted`。
    open_turns: HashMap<i64, Vec<usize>>,
    /// 本文件已产出的归一化事件列表。
    events: Vec<UsageEvent>,
}

impl FileState {
    fn new(fallback_session: String) -> Self {
        Self {
            session_id: fallback_session,
            provider: None,
            model: "unknown".to_string(),
            seed_length: 0,
            seen_seqs: HashSet::new(),
            step_starts: HashMap::new(),
            open_turns: HashMap::new(),
            events: Vec::new(),
        }
    }
}

/// 解析 dsh 会话根目录（优先 `$DSH_HOME/sessions`，缺省 `~/.dsh/sessions`）。
pub fn default_sessions_dir() -> Option<PathBuf> {
    if let Some(dsh_home) = env::var_os("DSH_HOME") {
        if !dsh_home.is_empty() {
            let mut path = PathBuf::from(dsh_home);
            path.push("sessions");
            return Some(path);
        }
    }

    let home = env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".dsh");
    path.push("sessions");
    Some(path)
}

/// 扫描默认 dsh 目录下与 `[range.start_ms, range.end_ms)` 相交的 v4 会话文件；目录不存在返回空列表。
pub fn scan(range: TimeRange, catalog: &PricingCatalog) -> Result<Vec<UsageEvent>> {
    let root = match default_sessions_dir() {
        Some(dir) => dir,
        None => return Ok(Vec::new()),
    };
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    scan_dir(&root, range, catalog)
}

/// 扫描指定 dsh 会话根目录。
pub fn scan_dir(
    root: &Path,
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let mut all_events = Vec::new();
    let project_entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Ok(all_events),
    };

    for project_entry in project_entries.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let session_entries = match fs::read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for session_entry in session_entries.flatten() {
            let session_path = session_entry.path();
            if !session_path.is_dir() {
                continue;
            }
            let file_entries = match fs::read_dir(&session_path) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for file_entry in file_entries.flatten() {
                let file_path = file_entry.path();
                let filename = match file_path.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name,
                    None => continue,
                };
                // 只解析 v4 格式，旧格式与 v3 静默跳过
                if !filename.ends_with(".v4.jsonl.zstd") {
                    continue;
                }
                if !file_may_overlap(&file_path, range.start_ms) {
                    continue;
                }

                if let Ok(mut events) = scan_file(&file_path, filename, range, catalog) {
                    all_events.append(&mut events);
                }
            }
        }
    }

    Ok(all_events)
}

/// 利用文件最后修改时间剪枝明显早于查询起点的文件。
fn file_may_overlap(path: &Path, range_start_ms: i64) -> bool {
    let mtime = match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let mtime_ms = match mtime.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => return false,
    };
    mtime_ms >= range_start_ms
}

/// 进程内流式解压并解析单个 `.v4.jsonl.zstd` 文件。
pub fn scan_file(
    path: &Path,
    fallback_session: &str,
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(BufReader::new(file))?;
    let reader = BufReader::new(decoder);

    let mut state = FileState::new(fallback_session.to_string());

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }
        process_line(&line, &mut state, range, catalog);
    }

    Ok(state.events)
}

/// 处理单行 JSONL 事件并更新文件解析状态。
fn process_line(
    line: &str,
    state: &mut FileState,
    range: TimeRange,
    catalog: &PricingCatalog,
) {
    let ev: RawEvent = match serde_json::from_str(line) {
        Ok(parsed) => parsed,
        Err(_) => return,
    };

    let kind = match ev.kind.as_deref() {
        Some(k) => k,
        None => return,
    };

    match kind {
        "session" => {
            if let Some(id) = ev.id {
                state.session_id = id;
            }
            if let Some(seed) = ev.seed_length {
                if seed > 0 {
                    state.seed_length = seed;
                }
            }
            return;
        }
        "request/context" => {
            if let Some(ref data) = ev.data {
                if let Some(p) = data.get("provider").and_then(Value::as_str) {
                    state.provider = Some(p.to_string());
                }
                if let Some(m) = data.get("model").and_then(Value::as_str) {
                    state.model = m.to_string();
                }
            }
            return;
        }
        "request/header" => {
            if let Some(cfg) = ev
                .data
                .as_ref()
                .and_then(|d| d.get("header"))
                .and_then(|h| h.get("config"))
            {
                if let Some(p) = cfg.get("provider").and_then(Value::as_str) {
                    state.provider = Some(p.to_string());
                }
                if let Some(m) = cfg.get("model").and_then(Value::as_str) {
                    state.model = m.to_string();
                }
            }
            return;
        }
        "step/start" => {
            if let (Some(ts), Some(key)) = (ev.time, extract_step_key(ev.data.as_ref())) {
                state.step_starts.insert(key, ts);
            }
            return;
        }
        "turn/end" => {
            if let Some(ref data) = ev.data {
                if let Some(turn) = data.get("turn").and_then(Value::as_i64) {
                    let is_aborted = data
                        .get("reason")
                        .and_then(|r| r.get("kind"))
                        .and_then(Value::as_str)
                        == Some("aborted");

                    if let Some(indices) = state.open_turns.remove(&turn) {
                        if is_aborted {
                            for idx in indices {
                                if let Some(event) = state.events.get_mut(idx) {
                                    event.aborted = true;
                                }
                            }
                        }
                    }
                }
            }
            return;
        }
        "assistant/message" => {}
        _ => return,
    }

    // 跳过 fork 继承的前缀事件以及重复序号事件
    if let Some(seq) = ev.seq {
        if state.seed_length > 0 && seq <= state.seed_length {
            return;
        }
        if !state.seen_seqs.insert(seq) {
            return;
        }
    }

    let ts_ms = match ev.time {
        Some(t) => t,
        None => return,
    };
    if ts_ms < range.start_ms || ts_ms >= range.end_ms {
        return;
    }

    let data = match ev.data.as_ref() {
        Some(d) => d,
        None => return,
    };
    let usage_val = match data.get("usage") {
        Some(u) => u,
        None => return,
    };

    let usage = Usage {
        input: get_u64(usage_val, "inputTokens"),
        output: get_u64(usage_val, "outputTokens"),
        reasoning: get_u64(usage_val, "reasoningTokens"),
        cache_read: get_u64(usage_val, "cacheReadTokens"),
        cache_write: get_u64(usage_val, "cacheWriteTokens"),
    };
    if usage.total() == 0 {
        return;
    }

    let identity = catalog.identify(&state.model);
    let cost_usd = identity
        .pricing
        .as_ref()
        .map(|pricing| calc_cost_usd(pricing, &usage));

    let step_key = extract_step_key(Some(data));
    let (ttft_ms, duration_ms) = extract_stream_timing(data, step_key, &state.step_starts);

    let turn_num = data.get("turn").and_then(Value::as_i64);
    let turn_str = turn_num.map(|t| t.to_string());

    let event = UsageEvent {
        agent: Agent::Dsh,
        session: state.session_id.clone(),
        provider: state.provider.clone(),
        model: identity.name,
        ts_ms,
        usage,
        cost_usd,
        pricing: identity.pricing,
        ttft_ms,
        duration_ms,
        role: None,
        turn: turn_str,
        aborted: false,
    };

    let event_idx = state.events.len();
    state.events.push(event);

    if let Some(turn) = turn_num {
        state.open_turns.entry(turn).or_default().push(event_idx);
    }
}

/// 从事件 `data` 中提取 `(turn, step)` 键。
fn extract_step_key(data: Option<&Value>) -> Option<(i64, i64)> {
    let data = data?;
    let turn = data.get("turn")?.as_i64()?;
    let step = data.get("step")?.as_i64()?;
    Some((turn, step))
}

/// 严格从 `data.stream[]` 分块时间戳提取 TTFT 与纯解码窗口。
fn extract_stream_timing(
    data: &Value,
    step_key: Option<(i64, i64)>,
    step_starts: &HashMap<(i64, i64), i64>,
) -> (Option<u64>, Option<u64>) {
    let chunks = match data.get("stream").and_then(Value::as_array) {
        Some(arr) => arr,
        None => return (None, None),
    };

    let mut min_ts: Option<i64> = None;
    let mut max_ts: Option<i64> = None;
    let mut count: usize = 0;

    for chunk in chunks {
        let ts = match chunk.get("time").and_then(Value::as_i64) {
            Some(t) => t,
            None => continue,
        };
        count += 1;
        min_ts = match min_ts {
            Some(prev) => Some(prev.min(ts)),
            None => Some(ts),
        };
        max_ts = match max_ts {
            Some(prev) => Some(prev.max(ts)),
            None => Some(ts),
        };
    }

    let first = match min_ts {
        Some(f) => f,
        None => return (None, None),
    };

    let mut ttft_ms = None;
    if let Some(key) = step_key {
        if let Some(&step_start) = step_starts.get(&key) {
            if first >= step_start {
                ttft_ms = Some((first - step_start) as u64);
            }
        }
    }

    let mut duration_ms = None;
    if count >= 2 {
        if let Some(last) = max_ts {
            if last > first {
                duration_ms = Some((last - first) as u64);
            }
        }
    }

    (ttft_ms, duration_ms)
}

/// 从 JSON 对象安全读取非负整数字段，缺省为 0。
fn get_u64(obj: &Value, key: &str) -> u64 {
    obj.get(key).and_then(Value::as_u64).unwrap_or(0)
}
