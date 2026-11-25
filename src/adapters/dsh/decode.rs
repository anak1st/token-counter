//! dsh v4 JSONL 行解码层（纯函数，无状态）。
//!
//! 通过字节预过滤只对关心的事件执行 serde 反序列化，90%+ 无关行不进 JSON 解析器，
//! 命中行自动跳过 `data.message` 等大字段。

use std::str;

use serde::Deserialize;
use serde_json::from_str;

use crate::core::model::Usage;

/// 从 dsh v4 事件流解码出的强类型语义事件（已屏蔽原始 JSON 嵌套差异）。
#[derive(Debug, PartialEq)]
pub(super) enum DshEvent {
    Session {
        id: Option<String>,
        cwd: Option<String>,
        seed_length: i64,
    },
    /// 会话名称变更（后续 LLM 标题覆盖早期 fallback 标题）。
    SessionTitle {
        title: Option<String>,
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
struct RawSessionTitle {
    data: Option<RawSessionTitleData>,
}

#[derive(Deserialize)]
struct RawSessionTitleData {
    title: Option<String>,
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
    str::from_utf8(&bytes[start..i]).ok()
}

/// 将单行 JSONL 解码为语义清晰的 `DshEvent`；不关心的事件或格式异常行返回 `None`。
pub(super) fn parse_event_line(line: &str) -> Option<DshEvent> {
    let kind = peek_event_type(line)?;

    match kind {
        "session" => {
            let raw: RawSession = from_str(line).ok()?;
            Some(DshEvent::Session {
                id: raw.id,
                cwd: raw.cwd,
                seed_length: raw.seed_length.max(0),
            })
        }
        "session/title" => {
            let raw: RawSessionTitle = from_str(line).ok()?;
            let title = raw.data.and_then(|d| d.title);
            Some(DshEvent::SessionTitle { title })
        }
        "request/context" => {
            let raw: RawRequestContext = from_str(line).ok()?;
            let data = raw.data?;
            Some(DshEvent::Route {
                provider: data.provider,
                model: data.model,
            })
        }
        "request/header" => {
            let raw: RawRequestHeader = from_str(line).ok()?;
            let cfg = raw.data?.header?.config?;
            Some(DshEvent::Route {
                provider: cfg.provider,
                model: cfg.model,
            })
        }
        "step/start" => {
            let raw: RawStepStart = from_str(line).ok()?;
            let ts_ms = raw.time?;
            let data = raw.data?;
            let key = (data.turn?, data.step?);
            Some(DshEvent::StepStart { key, ts_ms })
        }
        "turn/end" => {
            let raw: RawTurnEnd = from_str(line).ok()?;
            let data = raw.data?;
            let turn = data.turn?;
            let aborted = data.reason.and_then(|r| r.kind).as_deref() == Some("aborted");
            Some(DshEvent::TurnEnd { turn, aborted })
        }
        "assistant/message" => {
            let raw: RawAssistantMessage = from_str(line).ok()?;
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
}
