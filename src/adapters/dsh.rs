//! DeepSeek Harness（dsh）v4 zstd JSONL adapter。
//!
//! 清晰分为三层，职责互不交叉：
//! 1. **解码层**（`parse_event_line` -> `DshEvent`）：字节预过滤 + 强类型反序列化，
//!    90%+ 无关行不进 JSON 解析器，命中的行自动跳过 `data.message` 等大字段。
//! 2. **状态机层**（`SessionParser`）：纯内存消费 `DshEvent`，维护路由、step 起点、
//!    fork 继承前缀跳过与 turn 中止回填，不涉及文件 I/O 或 JSON 细节。
//! 3. **调度层**（`scan`）：按 `ScanQuery` 筛选候选文件，用 `std::thread::scope` 多核并行解析。

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde::Deserialize;

use crate::adapters::ScanQuery;
use crate::model::{Agent, Usage, UsageEvent};
use crate::pricing::{calc_cost_usd, PricingCatalog};
use crate::time::TimeRange;

/// 解压输出读缓冲大小（64 KB）。
const DECODE_BUF_BYTES: usize = 64 * 1024;

// ============================================================================
// 第一层：JSONL 行解码（纯函数，无状态）
// ============================================================================

/// 从 dsh v4 事件流解码出的强类型语义事件（已屏蔽原始 JSON 嵌套差异）。
#[derive(Debug, PartialEq)]
enum DshEvent {
    Session {
        id: Option<String>,
        cwd: Option<String>,
        seed_length: i64,
    },
    /// 请求路由变更（统一收敛 `request/context` 与 `request/header`）。
    Route {
        provider: Option<String>,
        model: Option<String>,
    },
    StepStart {
        key: (i64, i64),
        ts_ms: i64,
    },
    TurnEnd {
        turn: i64,
        aborted: bool,
    },
    AssistantMessage {
        seq: Option<i64>,
        ts_ms: i64,
        turn: Option<i64>,
        step: Option<i64>,
        usage: Usage,
        stream_times: Vec<i64>,
    },
}

#[derive(Deserialize)]
struct RawSession {
    id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "seedLength", default)]
    seed_length: i64,
}

#[derive(Deserialize)]
struct RawProviderModel {
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct RawRequestContext {
    data: Option<RawProviderModel>,
}

#[derive(Deserialize)]
struct RawRequestHeader {
    data: Option<RawHeaderOuter>,
}

#[derive(Deserialize)]
struct RawHeaderOuter {
    header: Option<RawHeaderInner>,
}

#[derive(Deserialize)]
struct RawHeaderInner {
    config: Option<RawProviderModel>,
}

#[derive(Deserialize)]
struct RawStepStart {
    time: Option<i64>,
    data: Option<RawStepKey>,
}

#[derive(Deserialize)]
struct RawStepKey {
    turn: Option<i64>,
    step: Option<i64>,
}

#[derive(Deserialize)]
struct RawTurnEnd {
    data: Option<RawTurnEndData>,
}

#[derive(Deserialize)]
struct RawTurnEndData {
    turn: Option<i64>,
    reason: Option<RawTurnReason>,
}

#[derive(Deserialize)]
struct RawTurnReason {
    kind: Option<String>,
}

#[derive(Deserialize)]
struct RawAssistantMessage {
    seq: Option<i64>,
    time: Option<i64>,
    data: Option<RawAssistantData>,
}

#[derive(Deserialize)]
struct RawAssistantData {
    turn: Option<i64>,
    step: Option<i64>,
    usage: Option<RawUsage>,
    stream: Option<Vec<RawStreamChunk>>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(rename = "inputTokens", default)]
    input_tokens: u64,
    #[serde(rename = "outputTokens", default)]
    output_tokens: u64,
    #[serde(rename = "reasoningTokens", default)]
    reasoning_tokens: u64,
    #[serde(rename = "cacheReadTokens", default)]
    cache_read_tokens: u64,
    #[serde(rename = "cacheWriteTokens", default)]
    cache_write_tokens: u64,
}

#[derive(Deserialize)]
struct RawStreamChunk {
    time: Option<i64>,
}

/// 快速读取单行 JSON 顶层的 `"type"` 值；无关事件不进入 JSON 解析器。
fn peek_event_type(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let needle = b"\"type\"";
    let search_end = bytes.len().min(256);
    let pos = bytes[..search_end]
        .windows(needle.len())
        .position(|w| w == needle)?;

    let mut i = pos + needle.len();
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b':' {
        return None;
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    i += 1;
    let start = i;
    while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\\' {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    std::str::from_utf8(&bytes[start..i]).ok()
}

/// 将单行 JSONL 解码为语义清晰的 `DshEvent`；不关心的事件或格式异常行返回 `None`。
fn parse_event_line(line: &str) -> Option<DshEvent> {
    let kind = peek_event_type(line)?;

    match kind {
        "session" => {
            let raw: RawSession = serde_json::from_str(line).ok()?;
            Some(DshEvent::Session {
                id: raw.id,
                cwd: raw.cwd,
                seed_length: raw.seed_length.max(0),
            })
        }
        "request/context" => {
            let raw: RawRequestContext = serde_json::from_str(line).ok()?;
            let data = raw.data?;
            Some(DshEvent::Route {
                provider: data.provider,
                model: data.model,
            })
        }
        "request/header" => {
            let raw: RawRequestHeader = serde_json::from_str(line).ok()?;
            let cfg = raw.data?.header?.config?;
            Some(DshEvent::Route {
                provider: cfg.provider,
                model: cfg.model,
            })
        }
        "step/start" => {
            let raw: RawStepStart = serde_json::from_str(line).ok()?;
            let ts_ms = raw.time?;
            let data = raw.data?;
            let key = (data.turn?, data.step?);
            Some(DshEvent::StepStart { key, ts_ms })
        }
        "turn/end" => {
            let raw: RawTurnEnd = serde_json::from_str(line).ok()?;
            let data = raw.data?;
            let turn = data.turn?;
            let aborted = data.reason.and_then(|r| r.kind).as_deref() == Some("aborted");
            Some(DshEvent::TurnEnd { turn, aborted })
        }
        "assistant/message" => {
            let raw: RawAssistantMessage = serde_json::from_str(line).ok()?;
            let ts_ms = raw.time?;
            let data = raw.data?;
            let u = data.usage?;
            let usage = Usage {
                input: u.input_tokens,
                output: u.output_tokens,
                reasoning: u.reasoning_tokens,
                cache_read: u.cache_read_tokens,
                cache_write: u.cache_write_tokens,
            };
            let mut stream_times = Vec::new();
            if let Some(chunks) = data.stream {
                for chunk in chunks {
                    if let Some(t) = chunk.time {
                        stream_times.push(t);
                    }
                }
            }
            Some(DshEvent::AssistantMessage {
                seq: raw.seq,
                ts_ms,
                turn: data.turn,
                step: data.step,
                usage,
                stream_times,
            })
        }
        _ => None,
    }
}

// ============================================================================
// 第二层：单会话纯状态机（无 I/O、无 JSON）
// ============================================================================

/// 消费 `DshEvent` 流并产出归一化 `UsageEvent` 的纯状态机。
struct SessionParser {
    session_id: String,
    session_label: Option<String>,
    provider: Option<String>,
    model: String,
    seed_length: i64,
    seen_seqs: HashSet<i64>,
    step_starts: HashMap<(i64, i64), i64>,
    open_turns: HashMap<i64, Vec<usize>>,
    events: Vec<UsageEvent>,
}

impl SessionParser {
    fn new(fallback_session: String) -> Self {
        Self {
            session_id: fallback_session,
            session_label: None,
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
    fn apply(&mut self, event: DshEvent, range: TimeRange, catalog: &PricingCatalog) {
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
                    self.session_label = extract_dir_name(&cwd);
                }
                if seed_length > 0 {
                    self.seed_length = seed_length;
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
            session_label: self.session_label.clone(),
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

    fn into_events(self) -> Vec<UsageEvent> {
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

/// 从工作目录路径提取末级目录名作为会话标签（如 `/Users/me/Code/app` -> `app`）。
fn extract_dir_name(cwd: &str) -> Option<String> {
    let name = Path::new(cwd).file_name()?.to_str()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

// ============================================================================
// 第三层：文件发现、查询路由与多核并行调度
// ============================================================================

/// 单个待扫描的 v4 会话文件元信息。
#[derive(Debug, Clone)]
struct CandidateFile {
    path: PathBuf,
    filename: String,
    dir_name: String,
    mtime_ms: i64,
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

/// 统一查询入口：按 `ScanQuery` 执行最合适的剪枝与并行扫描策略。
pub fn scan(query: &ScanQuery, catalog: &PricingCatalog) -> Result<Vec<UsageEvent>> {
    let root = match default_sessions_dir() {
        Some(dir) => dir,
        None => return Ok(Vec::new()),
    };
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    scan_dir(&root, query, catalog)
}

/// 在指定根目录上执行 `ScanQuery`。
pub fn scan_dir(
    root: &Path,
    query: &ScanQuery,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    match query {
        ScanQuery::Range(range) => {
            let candidates = collect_candidates(root, Some(range.start_ms));
            Ok(scan_files_parallel(&candidates, *range, catalog))
        }
        ScanQuery::RecentSessions { limit } => {
            if *limit == 0 {
                return Ok(Vec::new());
            }
            let candidates = collect_candidates(root, None);
            Ok(scan_recent_batches(&candidates, *limit, catalog))
        }
        ScanQuery::SessionPrefix(prefix) => {
            let candidates = collect_candidates(root, None);
            let selected = select_by_dir_prefix(&candidates, prefix);
            let target = if selected.is_empty() {
                candidates.as_slice()
            } else {
                selected.as_slice()
            };
            Ok(scan_files_parallel(target, TimeRange::ALL, catalog))
        }
    }
}

/// 按 `mtime` 倒序分批并行解压，凑够 `limit` 个非空会话即停止。
fn scan_recent_batches(
    candidates: &[CandidateFile],
    limit: usize,
    catalog: &PricingCatalog,
) -> Vec<UsageEvent> {
    let mut events = Vec::new();
    let mut seen_sessions: HashSet<String> = HashSet::new();
    let batch_size = (limit + 8).max(16);
    let mut offset = 0usize;

    while offset < candidates.len() && seen_sessions.len() < limit {
        let end = (offset + batch_size).min(candidates.len());
        let mut batch = scan_files_parallel(&candidates[offset..end], TimeRange::ALL, catalog);
        for ev in &batch {
            seen_sessions.insert(ev.session.clone());
        }
        events.append(&mut batch);
        offset = end;
    }

    events
}

/// 优先按会话目录名前缀过滤候选文件。
fn select_by_dir_prefix(candidates: &[CandidateFile], prefix: &str) -> Vec<CandidateFile> {
    let prefix_lower = prefix.trim().to_ascii_lowercase();
    if prefix_lower.is_empty() {
        return Vec::new();
    }
    let mut matched = Vec::new();
    for item in candidates {
        if item.dir_name.to_ascii_lowercase().starts_with(&prefix_lower) {
            matched.push(item.clone());
        }
    }
    matched
}

/// 收集目录下全部 `.v4.jsonl.zstd` 候选文件并按 `mtime_ms` 倒序排列。
fn collect_candidates(root: &Path, min_mtime_ms: Option<i64>) -> Vec<CandidateFile> {
    let mut candidates = Vec::new();
    let project_entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return candidates,
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
            let dir_name = session_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let file_entries = match fs::read_dir(&session_path) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for file_entry in file_entries.flatten() {
                let file_path = file_entry.path();
                let filename = match file_path.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name.to_string(),
                    None => continue,
                };
                if !filename.ends_with(".v4.jsonl.zstd") {
                    continue;
                }
                let mtime_ms = match file_mtime_ms(&file_path) {
                    Some(ms) => ms,
                    None => continue,
                };
                if let Some(min_ms) = min_mtime_ms {
                    if mtime_ms < min_ms {
                        continue;
                    }
                }

                candidates.push(CandidateFile {
                    path: file_path,
                    filename,
                    dir_name: dir_name.clone(),
                    mtime_ms,
                });
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.mtime_ms
            .cmp(&a.mtime_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    candidates
}

/// 读取文件修改时间（毫秒）。
fn file_mtime_ms(path: &Path) -> Option<i64> {
    let mtime = fs::metadata(path).ok()?.modified().ok()?;
    let dur = mtime.duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as i64)
}

/// 使用标准库 `thread::scope` + 原子游标动态抢占并行解析多个文件。
fn scan_files_parallel(
    candidates: &[CandidateFile],
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Vec<UsageEvent> {
    if candidates.is_empty() {
        return Vec::new();
    }
    if candidates.len() <= 2 {
        let mut out = Vec::new();
        for item in candidates {
            if let Ok(mut events) = scan_file(&item.path, &item.filename, range, catalog) {
                out.append(&mut events);
            }
        }
        return out;
    }

    let cores = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let worker_count = cores.min(candidates.len());
    let next_idx = AtomicUsize::new(0);
    let collected: Mutex<Vec<UsageEvent>> = Mutex::new(Vec::new());

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let next_ref = &next_idx;
            let collected_ref = &collected;
            scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    let idx = next_ref.fetch_add(1, Ordering::Relaxed);
                    if idx >= candidates.len() {
                        break;
                    }
                    let item = &candidates[idx];
                    if let Ok(mut events) = scan_file(&item.path, &item.filename, range, catalog) {
                        local.append(&mut events);
                    }
                }
                if !local.is_empty() {
                    if let Ok(mut guard) = collected_ref.lock() {
                        guard.append(&mut local);
                    }
                }
            });
        }
    });

    collected.into_inner().unwrap_or_default()
}

/// 流式解压并解析单个 `.v4.jsonl.zstd` 文件。
pub fn scan_file(
    path: &Path,
    fallback_session: &str,
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(BufReader::new(file))?;
    let mut reader = BufReader::with_capacity(DECODE_BUF_BYTES, decoder);

    let mut parser = SessionParser::new(fallback_session.to_string());
    let mut line = String::with_capacity(4096);

    loop {
        line.clear();
        let bytes_read = match reader.read_line(&mut line) {
            Ok(n) => n,
            Err(_) => break,
        };
        if bytes_read == 0 {
            break;
        }
        if let Some(event) = parse_event_line(&line) {
            parser.apply(event, range, catalog);
        }
    }

    Ok(parser.into_events())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_events_and_skips_noise_lines() {
        assert!(parse_event_line(r#"{"type":"tool/result","data":{"content":"huge"}}"#).is_none());

        let route = parse_event_line(
            r#"{"type":"request/context","data":{"provider":"opencode-go","model":"deepseek-flash"}}"#,
        );
        assert_eq!(
            route,
            Some(DshEvent::Route {
                provider: Some("opencode-go".to_string()),
                model: Some("deepseek-flash".to_string()),
            })
        );

        let turn_end =
            parse_event_line(r#"{"type":"turn/end","data":{"turn":3,"reason":{"kind":"aborted"}}}"#);
        assert_eq!(
            turn_end,
            Some(DshEvent::TurnEnd {
                turn: 3,
                aborted: true,
            })
        );
    }

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
        ];

        for ev in stream {
            parser.apply(ev, TimeRange::ALL, &catalog);
        }

        let events = parser.into_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session, "session-test");
        assert_eq!(events[0].session_label.as_deref(), Some("demo"));
        assert_eq!(events[0].ttft_ms, Some(1_500));
        assert_eq!(events[0].duration_ms, Some(2_000));
        assert!(events[0].aborted);
    }
}
