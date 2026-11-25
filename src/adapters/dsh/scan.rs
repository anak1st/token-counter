//! dsh 文件发现、查询路由与多核并行解压调度层。

use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use zstd::stream::read::Decoder;

use super::decode::parse_event_line;
use super::parser::SessionParser;
use crate::adapters::ScanQuery;
use crate::core::model::UsageEvent;
use crate::core::pricing::PricingCatalog;
use crate::core::time::TimeRange;

/// 解压输出读缓冲大小（64 KB）。
const DECODE_BUF_BYTES: usize = 64 * 1024;

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
    let decoder = Decoder::new(BufReader::new(file))?;
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
