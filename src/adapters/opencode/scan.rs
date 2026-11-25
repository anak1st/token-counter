//! OpenCode 只读 SQLite 查询层。
//!
//! 读取 `~/.local/share/opencode/opencode.db` 中的 `session_message` 与 `session_v2` 表：
//! - 每条 `type = 'assistant'` 记录对应一次模型请求，`user` 行用于切分 Turn；
//! - `tokens.input` 不含 `cache.read`，计价无需扣减；
//! - `ttft_ms`：首个 reasoning part 的 `time.created` 减去消息 `time.created`（近似值）；
//! - `duration_ms`：`time.streamed` 减去首个 reasoning part 的 `time.created`（纯解码窗口，不含工具执行）。

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};

use super::decode::{build_event, MessageData};
use crate::core::format::{folder_name_from_path, non_empty_trimmed};
use crate::core::model::UsageEvent;
use crate::core::pricing::PricingCatalog;
use crate::core::time::TimeRange;

/// 从 `session_v2` 提取的会话展示元信息。
#[derive(Default)]
struct SessionMeta {
    name: Option<String>,
    folder: Option<String>,
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

    let metas = load_session_meta(&conn);

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
        let meta = metas.get(&session_id);
        let session_name = meta.and_then(|m| m.name.clone());
        let folder = meta.and_then(|m| m.folder.clone());

        if let Some(event) = build_event(
            session_id,
            session_name,
            folder,
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

/// 从 `session_v2` 读取会话名称（`title`）与工作目录末级文件夹名（`directory`），不读取汇总 token。
fn load_session_meta(conn: &Connection) -> HashMap<String, SessionMeta> {
    let mut metas = HashMap::new();
    let mut stmt = match conn.prepare("SELECT id, title, directory FROM session_v2") {
        Ok(s) => s,
        Err(_) => return metas,
    };

    let rows = match stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let directory: Option<String> = row.get(2)?;
        Ok((id, title, directory))
    }) {
        Ok(r) => r,
        Err(_) => return metas,
    };

    for (id, title, directory) in rows.flatten() {
        let name = title.as_deref().and_then(non_empty_trimmed);
        let folder = directory.as_deref().and_then(folder_name_from_path);
        if name.is_some() || folder.is_some() {
            metas.insert(id, SessionMeta { name, folder });
        }
    }

    metas
}
