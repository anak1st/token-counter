//! OpenCode `session_message.data` JSON 反序列化与归一化事件构造。

use serde::Deserialize;
use serde_json::Value;

use crate::core::model::{Agent, Usage, UsageEvent};
use crate::core::pricing::{calc_cost_usd, PricingCatalog};

/// `session_message.data` JSON 中本 adapter 关心的字段子集。
#[derive(Debug, Deserialize)]
pub(super) struct MessageData {
    pub time: Option<MessageTime>,
    pub agent: Option<String>,
    pub model: Option<MessageModel>,
    pub content: Option<Vec<MessagePart>>,
    pub finish: Option<String>,
    pub error: Option<Value>,
    pub tokens: Option<MessageTokens>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageTime {
    pub created: Option<i64>,
    pub streamed: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageModel {
    pub id: Option<String>,
    #[serde(rename = "providerID")]
    pub provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessagePart {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub time: Option<PartTime>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PartTime {
    pub created: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageTokens {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub reasoning: Option<u64>,
    pub cache: Option<MessageCache>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageCache {
    pub read: Option<u64>,
    pub write: Option<u64>,
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
pub(super) fn build_event(
    session_id: String,
    session_name: Option<String>,
    folder: Option<String>,
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
        session_name,
        folder,
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
