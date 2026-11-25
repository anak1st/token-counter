//! Antigravity（agy）只读 SQLite + protobuf adapter。
//!
//! 扫描 `~/.gemini/antigravity/conversations/*.db`：
//! - 主表：`steps`（`step_type = 15` 对应 `PLANNER_RESPONSE`）与 `gen_metadata`（模型 ID、TTFT、纯生成耗时）。
//! - token 口径：
//!   - `f2` 为未命中缓存输入（不含 `cache_read`，计价无需扣减）；
//!   - `f5` 为 `cache_read`；
//!   - `f9`（reasoning）与 `f10`（非思考输出）互斥且满足 `f9 + f10 == f3`，不满足时全部留在 `output`。
//! - 只读 WAL 打开策略：
//!   - 活跃库存在非空 `-wal` 时走普通只读以读取未 checkpoint 的增量；
//!   - 已关闭无 `-wal` 的库走 `file:<path>?immutable=1` 绕过缺少 `-shm` 的打开错误。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::from_str;

use super::proto::{decode_wire, get_bytes, get_string, get_u64, parse_time_ms};
use crate::core::format::{folder_name_from_path, non_empty_trimmed};
use crate::core::model::{Agent, Usage, UsageEvent};
use crate::core::paths::user_home_dir;
use crate::core::pricing::{calc_cost_usd, PricingCatalog};
use crate::core::time::TimeRange;

/// 从 `conversation_summaries.db` 提取的会话展示元信息。
#[derive(Default)]
struct SessionMeta {
    name: Option<String>,
    folder: Option<String>,
}

const STEP_TYPE_PLANNER: i64 = 15;
const STEP_STATUS_DONE: i64 = 3;

/// 解析后的 token 用量子消息。
#[derive(Debug, Clone, Copy)]
struct ParsedUsage {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
}

/// 从单条 `gen_metadata.data` 提取的模型与速度元数据。
struct GenInfo {
    model: String,
    last_step_index: Option<i64>,
    ttft_ms: Option<u64>,
    duration_ms: Option<u64>,
    usage: Option<ParsedUsage>,
}

/// 返回 Antigravity 会话库默认目录：`~/.gemini/antigravity/conversations`（兼容 Windows `USERPROFILE`）。
pub fn default_conversations_dir() -> Option<PathBuf> {
    Some(
        user_home_dir()?
            .join(".gemini")
            .join("antigravity")
            .join("conversations"),
    )
}

/// 扫描默认目录下与 `[range.start_ms, range.end_ms)` 相交的全部会话库；目录不存在返回空列表。
pub fn scan(range: TimeRange, catalog: &PricingCatalog) -> Result<Vec<UsageEvent>> {
    let root = match default_conversations_dir() {
        Some(dir) => dir,
        None => return Ok(Vec::new()),
    };
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    scan_dir(&root, range, catalog)
}

/// 扫描指定目录内的全部 `*.db` 会话库。
pub fn scan_dir(
    root: &Path,
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let summaries = load_conversation_summaries(root);
    let mut all_events = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Ok(all_events),
    };

    for entry in entries.flatten() {
        let db_path = entry.path();
        let name = match db_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.ends_with(".db") {
            continue;
        }
        if !db_may_overlap(&db_path, range.start_ms) {
            continue;
        }

        let fallback_session = name.trim_end_matches(".db");
        if let Ok(mut events) = scan_conversation(&db_path, fallback_session, range, catalog) {
            for ev in &mut events {
                if let Some(meta) = summaries.get(&ev.session) {
                    ev.session_name = meta.name.clone();
                    ev.folder = meta.folder.clone();
                }
            }
            all_events.append(&mut events);
        }
    }

    Ok(all_events)
}

/// 取 `.db` 与 `.db-wal` 的最大修改时间进行区间前剪枝。
fn db_may_overlap(db_path: &Path, range_start_ms: i64) -> bool {
    let mut latest = match file_mtime_ms(db_path) {
        Some(ms) => ms,
        None => return false,
    };

    let wal_path = sidecar_path(db_path, "-wal");
    if let Some(wal_ms) = file_mtime_ms(&wal_path) {
        if wal_ms > latest {
            latest = wal_ms;
        }
    }

    latest >= range_start_ms
}

/// 读取文件修改时间（毫秒）。
fn file_mtime_ms(path: &Path) -> Option<i64> {
    let mtime = fs::metadata(path).ok()?.modified().ok()?;
    let dur = mtime.duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as i64)
}

/// 构造 SQLite 伴随文件路径（如 `<db>-wal`）。
fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// 严格只读打开 SQLite 库；有非空 `-wal` 时普通只读，否则使用 `immutable=1`。
fn open_readonly_sqlite(db_path: &Path) -> Result<Connection> {
    let wal_path = sidecar_path(db_path, "-wal");
    let has_wal = fs::metadata(&wal_path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false);

    if has_wal {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        return Ok(Connection::open_with_flags(db_path, flags)?);
    }

    let uri = build_immutable_uri(db_path);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Ok(Connection::open_with_flags(uri, flags)?)
}

/// 从同级 `conversation_summaries.db` 只读加载会话标题与工作区文件夹名；文件不存在或格式不符时返回空映射。
fn load_conversation_summaries(conversations_dir: &Path) -> HashMap<String, SessionMeta> {
    let mut map = HashMap::new();
    let summary_db = match conversations_dir.parent() {
        Some(parent) => parent.join("conversation_summaries.db"),
        None => return map,
    };
    if !summary_db.is_file() {
        return map;
    }

    let conn = match open_readonly_sqlite(&summary_db) {
        Ok(c) => c,
        Err(_) => return map,
    };

    let mut stmt = match conn
        .prepare("SELECT conversation_id, title, workspace_uris FROM conversation_summaries")
    {
        Ok(s) => s,
        Err(_) => return map,
    };

    let rows = match stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let workspace_uris: String = row.get(2)?;
        Ok((id, title, workspace_uris))
    }) {
        Ok(r) => r,
        Err(_) => return map,
    };

    for (id, title, workspace_uris) in rows.flatten() {
        let name = non_empty_trimmed(&title);
        let folder = parse_workspace_folder(&workspace_uris);
        if name.is_some() || folder.is_some() {
            map.insert(id, SessionMeta { name, folder });
        }
    }

    map
}

/// 从 `workspace_uris` JSON 数组提取首个本地工作区目录名（如 `["file:///Users/me/Code/app"]` -> `app`）。
fn parse_workspace_folder(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let uris: Vec<String> = from_str(trimmed).ok()?;
    for uri in uris {
        if let Some(path) = uri.strip_prefix("file://") {
            let decoded = percent_decode(path);
            if let Some(folder) = folder_name_from_path(&decoded) {
                return Some(folder);
            }
        }
    }
    None
}

/// 解码 URI 路径中的 `%XX` 百分号编码（兼容空格与中文目录名）。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 构造跨平台 SQLite 只读 `immutable=1` URI（兼容 Unix 路径、Windows 盘符与 UNC 路径）。
fn build_immutable_uri(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let encoded = encode_uri_chars(&normalized);

    if encoded.starts_with("//") {
        // UNC 路径 `\\server\share\...` -> `file://server/share/...`
        format!("file:{}?immutable=1", encoded)
    } else if encoded.starts_with('/') {
        // Unix 绝对路径 `/Users/...` -> `file:///Users/...`
        format!("file://{}?immutable=1", encoded)
    } else if encoded.as_bytes().get(1) == Some(&b':') {
        // Windows 盘符路径 `C:/Users/...` -> `file:///C:/Users/...`
        format!("file:///{}?immutable=1", encoded)
    } else {
        format!("file:{}?immutable=1", encoded)
    }
}

/// 对 SQLite `file:` URI 路径中的 `%`、`?`、`#` 做百分号编码。
fn encode_uri_chars(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            '#' => out.push_str("%23"),
            other => out.push(other),
        }
    }
    out
}

/// 扫描单个 Antigravity 会话库并产出区间内的归一化事件。
pub fn scan_conversation(
    db_path: &Path,
    fallback_session: &str,
    range: TimeRange,
    catalog: &PricingCatalog,
) -> Result<Vec<UsageEvent>> {
    let conn = open_readonly_sqlite(db_path)?;

    let mut session = fallback_session.to_string();
    if let Ok(Some(cascade_id)) = conn.query_row(
        "SELECT cascade_id FROM trajectory_meta LIMIT 1",
        [],
        |row| row.get::<_, Option<String>>(0),
    ) {
        if !cascade_id.is_empty() {
            session = cascade_id;
        }
    }

    let mut step_stmt = match conn
        .prepare("SELECT idx, status, metadata FROM steps WHERE step_type = ?1 ORDER BY idx")
    {
        Ok(stmt) => stmt,
        Err(_) => return Ok(Vec::new()),
    };

    let mut gen_stmt = match conn.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx") {
        Ok(stmt) => stmt,
        Err(_) => return Ok(Vec::new()),
    };

    let mut steps: Vec<(i64, i64, Vec<u8>)> = Vec::new();
    let step_rows = step_stmt.query_map(params![STEP_TYPE_PLANNER], |row| {
        let idx: i64 = row.get(0)?;
        let status: i64 = row.get(1)?;
        let metadata: Option<Vec<u8>> = row.get(2)?;
        Ok((idx, status, metadata))
    })?;
    for row_res in step_rows {
        let (idx, status, metadata) = row_res?;
        if let Some(bytes) = metadata {
            steps.push((idx, status, bytes));
        }
    }

    let mut gen_list: Vec<GenInfo> = Vec::new();
    let mut gen_by_step_idx: HashMap<i64, usize> = HashMap::new();
    let gen_rows = gen_stmt.query_map([], |row| {
        let data: Option<Vec<u8>> = row.get(1)?;
        Ok(data)
    })?;
    for row_res in gen_rows {
        let data = match row_res? {
            Some(bytes) => bytes,
            None => continue,
        };
        let parsed = parse_gen_metadata(&data);
        let gen_idx = gen_list.len();
        if let Some(last_step) = parsed.last_step_index {
            gen_by_step_idx.insert(last_step + 1, gen_idx);
        }
        gen_list.push(parsed);
    }

    let default_provider = Agent::Agy.default_provider().map(str::to_string);
    let mut events = Vec::new();

    for (pos, (step_idx, status, meta_bytes)) in steps.iter().enumerate() {
        let meta_fields = decode_wire(meta_bytes);
        let ts_ms = match parse_time_ms(get_bytes(&meta_fields, 1)) {
            Some(ts) => ts,
            None => continue,
        };
        if ts_ms < range.start_ms || ts_ms >= range.end_ms {
            continue;
        }

        let gen_info = gen_by_step_idx
            .get(step_idx)
            .and_then(|&idx| gen_list.get(idx))
            .or_else(|| gen_list.get(pos));

        let parsed_usage = parse_usage(get_bytes(&meta_fields, 9))
            .or_else(|| gen_info.and_then(|g| g.usage));
        let parsed_usage = match parsed_usage {
            Some(u) => u,
            None => continue,
        };

        let usage = Usage {
            input: parsed_usage.input,
            output: parsed_usage.output,
            reasoning: parsed_usage.reasoning,
            cache_read: parsed_usage.cache_read,
            cache_write: 0,
        };
        if usage.total() == 0 {
            continue;
        }

        let raw_model = gen_info.map(|g| g.model.as_str()).unwrap_or("unknown");
        let identity = catalog.identify(raw_model);
        let cost_usd = identity
            .pricing
            .as_ref()
            .map(|pricing| calc_cost_usd(pricing, &usage));

        let stream_start = parse_time_ms(get_bytes(&meta_fields, 6));
        let stream_end = parse_time_ms(get_bytes(&meta_fields, 7));

        let mut ttft_ms = gen_info.and_then(|g| g.ttft_ms);
        if ttft_ms.is_none() {
            if let Some(start_ts) = stream_start {
                if start_ts >= ts_ms {
                    ttft_ms = Some((start_ts - ts_ms) as u64);
                }
            }
        }

        let mut duration_ms = gen_info.and_then(|g| g.duration_ms);
        if duration_ms.is_none() {
            if let (Some(start_ts), Some(end_ts)) = (stream_start, stream_end) {
                if end_ts > start_ts {
                    duration_ms = Some((end_ts - start_ts) as u64);
                }
            }
        }

        let turn = get_string(&meta_fields, 12);
        let aborted = *status != STEP_STATUS_DONE;

        events.push(UsageEvent {
            agent: Agent::Agy,
            session: session.clone(),
            session_name: None,
            folder: None,
            provider: default_provider.clone(),
            model: identity.name,
            ts_ms,
            usage,
            cost_usd,
            pricing: identity.pricing,
            ttft_ms,
            duration_ms,
            role: None,
            turn,
            aborted,
        });
    }

    Ok(events)
}

/// 解码 usage 子消息（`f2=未命中输入`, `f3=总输出`, `f5=缓存命中`, `f9=思考`, `f10=非思考输出`）。
fn parse_usage(bytes: Option<&[u8]>) -> Option<ParsedUsage> {
    let bytes = bytes?;
    if bytes.is_empty() {
        return None;
    }
    let fields = decode_wire(bytes);
    let input = get_u64(&fields, 2).unwrap_or(0);
    let total_out = get_u64(&fields, 3).unwrap_or(0);
    let cache_read = get_u64(&fields, 5).unwrap_or(0);
    let reasoning = get_u64(&fields, 9);
    let direct_out = get_u64(&fields, 10);

    // 仅当 f9 + f10 精确等于 f3 时拆分思考 token，否则全部留在 output 保持互斥不变量
    if let (Some(r), Some(d)) = (reasoning, direct_out) {
        if r.saturating_add(d) == total_out {
            return Some(ParsedUsage {
                input,
                output: d,
                reasoning: r,
                cache_read,
            });
        }
    }

    Some(ParsedUsage {
        input,
        output: total_out,
        reasoning: 0,
        cache_read,
    })
}

/// 解析单条 `gen_metadata.data`，提取模型 ID、关联 step 索引、TTFT 与纯解码时长。
fn parse_gen_metadata(buf: &[u8]) -> GenInfo {
    let outer = decode_wire(buf);
    let inner_bytes = match get_bytes(&outer, 1) {
        Some(b) => b,
        None => {
            return GenInfo {
                model: "unknown".to_string(),
                last_step_index: None,
                ttft_ms: None,
                duration_ms: None,
                usage: None,
            };
        }
    };

    let inner = decode_wire(inner_bytes);
    let model = get_string(&inner, 19)
        .or_else(|| get_string(&inner, 21))
        .unwrap_or_else(|| "unknown".to_string());

    let mut ttft_ms = None;
    if let Some(ms) = parse_time_ms(get_bytes(&inner, 11)) {
        if ms >= 0 {
            ttft_ms = Some(ms as u64);
        }
    }

    let mut duration_ms = None;
    if let Some(ms) = parse_time_ms(get_bytes(&inner, 12)) {
        if ms > 0 {
            duration_ms = Some(ms as u64);
        }
    }

    let usage = parse_usage(get_bytes(&inner, 4));

    let mut last_step_index = None;
    for item in &inner {
        if item.field != 20 {
            continue;
        }
        let kv_bytes = match item.bytes {
            Some(b) => b,
            None => continue,
        };
        let kv = decode_wire(kv_bytes);
        if get_string(&kv, 1).as_deref() == Some("last_step_index") {
            if let Some(val) = get_string(&kv, 2) {
                if let Ok(parsed) = val.parse::<i64>() {
                    last_step_index = Some(parsed);
                }
            }
        }
    }

    GenInfo {
        model,
        last_step_index,
        ttft_ms,
        duration_ms,
        usage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_cross_platform_sqlite_immutable_uris() {
        assert_eq!(
            build_immutable_uri(Path::new("/Users/me/a#1.db")),
            "file:///Users/me/a%231.db?immutable=1"
        );
        assert_eq!(
            build_immutable_uri(Path::new(r"C:\Users\me\a%b?c.db")),
            "file:///C:/Users/me/a%25b%3Fc.db?immutable=1"
        );
        assert_eq!(
            build_immutable_uri(Path::new(r"\\server\share\a.db")),
            "file://server/share/a.db?immutable=1"
        );
    }

    #[test]
    fn parses_unix_and_windows_workspace_uris() {
        assert_eq!(
            parse_workspace_folder(r#"["file:///Users/me/Code/token-counter"]"#).as_deref(),
            Some("token-counter")
        );
        assert_eq!(
            parse_workspace_folder(r#"["file:///c%3A/Users/me/Code/my%20app"]"#).as_deref(),
            Some("my app")
        );
        assert_eq!(parse_workspace_folder(""), None);
    }
}
