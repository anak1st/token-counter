//! OpenCode 只读 SQLite adapter。
//!
//! 读取 `~/.local/share/opencode/opencode.db` 中的 `session_message` 表：
//! - 每条 `type = 'assistant'` 记录对应一次模型请求。
//! - `tokens.input` 不含 `cache.read`，计价无需扣减。
//! - `ttft_ms`：首个 reasoning part 的 `time.created` 减去消息 `time.created`（近似值）。
//! - `duration_ms`：`time.streamed` 减去首个 reasoning part 的 `time.created`（纯解码窗口，不含工具执行）。

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

    let mut stmt = conn.prepare(
        "SELECT session_id, time_created, data
         FROM session_message
         WHERE type = 'assistant' AND time_created >= ?1 AND time_created < ?2
         ORDER BY time_created",
    )?;

    let rows = stmt.query_map(params![range.start_ms, range.end_ms], |row| {
        let session_id: String = row.get(0)?;
        let time_created: i64 = row.get(1)?;
        let raw_data: String = row.get(2)?;
        Ok((session_id, time_created, raw_data))
    })?;

    let mut events = Vec::new();
    for row_result in rows {
        let (session_id, time_created, raw_data) = row_result?;
        let data: MessageData = match serde_json::from_str(&raw_data) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };

        if let Some(event) = build_event(session_id, time_created, data, catalog) {
            events.push(event);
        }
    }

    Ok(events)
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
        provider,
        model: identity.name,
        ts_ms,
        usage,
        cost_usd,
        pricing: identity.pricing,
        ttft_ms,
        duration_ms,
        role: data.agent,
        turn: None,
        aborted,
    })
}
