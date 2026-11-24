//! OpenCode 只读 SQLite adapter。
//!
//! 读取 `~/.local/share/opencode/opencode.db` 中的 `session_message` 表：
//! - 每条 `type = 'assistant'` 记录对应一次模型请求。
//! - `tokens.input` 不含 `cache.read`，计价无需扣减。
//! - `ttft_ms`：首个 reasoning part 的 `time.created` 减去消息 `time.created`（近似值）。
//! - `duration_ms`：`time.streamed` 减去首个 reasoning part 的 `time.created`（纯解码窗口，不含工具执行）。

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;

use crate::model::{Agent, Usage, UsageEvent};
use crate::pricing::{calc_cost_usd, PricingCatalog};
use crate::time::TimeRange;

/// `session_message.data` JSON 中本 adapter 关心的字段子集。
#[derive(Debug, Deserialize)]
struct MessageData {
    time: Option<MessageTime>,
    agent: Option<String>,
    model: Option<MessageModel>,
    content: Option<Vec<MessagePart>>,
    finish: Option<String>,
    error: Option<Value>,
    tokens: Option<MessageTokens>,
}

#[derive(Debug, Deserialize)]
struct MessageTime {
    created: Option<i64>,
    streamed: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MessageModel {
    id: Option<String>,
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessagePart {
    #[serde(rename = "type")]
    kind: Option<String>,
    time: Option<PartTime>,
}

#[derive(Debug, Deserialize)]
struct PartTime {
    created: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MessageTokens {
    input: Option<u64>,
    output: Option<u64>,
    reasoning: Option<u64>,
    cache: Option<MessageCache>,
}

#[derive(Debug, Deserialize)]
struct MessageCache {
    read: Option<u64>,
    write: Option<u64>,
}

/// 返回 OpenCode SQLite 默认路径：`~/.local/share/opencode/opencode.db`。
pub fn default_db_path() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".local");
    path.push("share");
    path.push("opencode");
    path.push("opencode.db");
    Some(path)
}

/// 扫描默认 OpenCode 数据库在 `[range.start_ms, range.end_ms)` 内的请求事件；库不存在时返回空列表。
pub fn scan(range: TimeRange, catalog: &PricingCatalog) -> Result<Vec<UsageEvent>> {
    let path = match default_db_path() {
        Some(path) => path,
        None => return Ok(Vec::new()),
    };
    if !path.is_file() {
        return Ok(Vec::new());
    }
    scan_path(&path, range, catalog)
}

/// 严格只读扫描指定路径的 OpenCode SQLite 数据库。
pub fn scan_path(
    path: &Path,
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("cannot open opencode db at {}", path.display()))?;

    let titles = load_session_titles(&conn);

    // 按 (session_id, seq, time_created) 同时读 user 与 assistant 行，用 user 行划分 turn
    let mut stmt = conn.prepare(
        "SELECT session_id, type, time_created, data
         FROM session_message
         WHERE type IN ('user', 'assistant') AND time_created >= ?1 AND time_created < ?2
         ORDER BY session_id, seq, time_created",
    )?;

    let rows = stmt.query_map(params![range.start_ms, range.end_ms], |row| {
        let session_id: String = row.get(0)?;
        let msg_type: String = row.get(1)?;
        let time_created: i64 = row.get(2)?;
        let raw_data: String = row.get(3)?;
        Ok((session_id, msg_type, time_created, raw_data))
    })?;

    let mut events = Vec::new();
    let mut turn_by_session: HashMap<String, u32> = HashMap::new();

    for row_result in rows {
        let (session_id, msg_type, time_created, raw_data) = row_result?;
        if msg_type == "user" {
            let counter = turn_by_session.entry(session_id).or_insert(0);
            *counter += 1;
            continue;
        }

        let data: MessageData = match serde_json::from_str(&raw_data) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };

        let turn_num = *turn_by_session.entry(session_id.clone()).or_insert(1);
        let session_label = titles.get(&session_id).cloned();

        if let Some(event) = build_event(
            session_id,
            session_label,
            turn_num.to_string(),
            time_created,
            data,
            catalog,
        ) {
            events.push(event);
        }
    }

    Ok(events)
}

/// 从 `session_v2` 读取非空会话标题（只取展示标签，不取其汇总 token）。
fn load_session_titles(conn: &Connection) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    let mut stmt = match conn.prepare("SELECT id, title FROM session_v2 WHERE title != ''") {
        Ok(s) => s,
        Err(_) => return titles,
    };

    let rows = match stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        Ok((id, title))
    }) {
        Ok(r) => r,
        Err(_) => return titles,
    };

    for item in rows.flatten() {
        let trimmed = item.1.trim();
        if !trimmed.is_empty() {
            titles.insert(item.0, trimmed.to_string());
        }
    }

    titles
}

/// 提取首个 reasoning part 的创建时刻作为流起点（text part 无时间戳，仅 reasoning 携带）。
fn first_reasoning_time(parts: Option<&[MessagePart]>) -> Option<i64> {
    let parts = parts?;
    let mut earliest: Option<i64> = None;

    for part in parts {
        if part.kind.as_deref() != Some("reasoning") {
            continue;
        }
        let created = match part.time.as_ref().and_then(|t| t.created) {
            Some(ts) => ts,
            None => continue,
        };
        earliest = match earliest {
            Some(prev) => Some(prev.min(created)),
            None => Some(created),
        };
    }

    earliest
}

/// 将单条解析后的 `session_message` 转为归一化 `UsageEvent`。
fn build_event(
    session_id: String,
    session_label: Option<String>,
    turn: String,
    row_time_created: i64,
    data: MessageData,
    catalog: &PricingCatalog,
) -> Option<UsageEvent> {
    let tokens = data.tokens?;
    let cache_read = tokens.cache.as_ref().and_then(|c| c.read).unwrap_or(0);
    let cache_write = tokens.cache.as_ref().and_then(|c| c.write).unwrap_or(0);

    let usage = Usage {
        input: tokens.input.unwrap_or(0),
        output: tokens.output.unwrap_or(0),
        reasoning: tokens.reasoning.unwrap_or(0),
        cache_read,
        cache_write,
    };
    if usage.total() == 0 {
        return None;
    }

    let raw_model = data
        .model
        .as_ref()
        .and_then(|m| m.id.as_deref())
        .unwrap_or("unknown");
    let identity = catalog.identify(raw_model);
    let cost_usd = identity
        .pricing
        .as_ref()
        .map(|pricing| calc_cost_usd(pricing, &usage));

    let ts_ms = data
        .time
        .as_ref()
        .and_then(|t| t.created)
        .unwrap_or(row_time_created);
    let first = first_reasoning_time(data.content.as_deref());
    let streamed = data.time.as_ref().and_then(|t| t.streamed);

    let mut ttft_ms = None;
    if let Some(first_ts) = first {
        if first_ts >= ts_ms {
            ttft_ms = Some((first_ts - ts_ms) as u64);
        }
    }

    let mut duration_ms = None;
    if let (Some(first_ts), Some(streamed_ts)) = (first, streamed) {
        if streamed_ts > first_ts {
            duration_ms = Some((streamed_ts - first_ts) as u64);
        }
    }

    let provider = data.model.and_then(|m| m.provider_id);
    let aborted = data.finish.as_deref() == Some("error")
        || matches!(data.error, Some(ref v) if !v.is_null());

    Some(UsageEvent {
        agent: Agent::Opencode,
        session: session_id,
        session_label,
        provider,
        model: identity.name,
        ts_ms,
        usage,
        cost_usd,
        pricing: identity.pricing,
        ttft_ms,
        duration_ms,
        role: data.agent,
        turn: Some(turn),
        aborted,
    })
}
