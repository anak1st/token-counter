//! 核心领域模型与内置 Agent 定义。
//!
//! Agent 强绑定各自的本地格式解析器（SQLite、zstd JSONL、protobuf），因此内化为
//! Rust 代码；模型元数据与官方美元价目表独立维护在 `config/models.json` 中。

use crate::pricing::ModelPricing;

/// 当前支持的 AI 编程 Agent 集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Opencode,
    Dsh,
    Agy,
}

impl Agent {
    /// 按展示顺序排列的全部内置 Agent。
    pub const ALL: [Agent; 3] = [Agent::Opencode, Agent::Dsh, Agent::Agy];

    /// 用于 CLI 参数和内部键的稳定小写标识。
    pub fn id(self) -> &'static str {
        match self {
            Agent::Opencode => "opencode",
            Agent::Dsh => "dsh",
            Agent::Agy => "agy",
        }
    }

    /// 用于表格与 TUI 徽章的简短展示名。
    pub fn label(self) -> &'static str {
        match self {
            Agent::Opencode => "OpenCode",
            Agent::Dsh => "DSH",
            Agent::Agy => "Antigravity",
        }
    }

    /// 源记录本身不带 provider 字段时的默认归属。
    pub fn default_provider(self) -> Option<&'static str> {
        match self {
            Agent::Opencode => None,
            Agent::Dsh => None,
            Agent::Agy => Some("google"),
        }
    }
}

/// 单次模型请求的 token 分项统计。
///
/// 不变量：
/// - `input` 仅包含未命中缓存的输入（源若合并了 `cache_read`，adapter 入口需扣减）。
/// - `reasoning` 与 `output` 互斥（源若为包含关系，adapter 入口需扣减或全部留在 `output`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    /// 全部 token 桶的总和。
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    /// 缓存相关 token（`cache_read + cache_write`），用于计算 `Cache%`。
    pub fn cache_tokens(&self) -> u64 {
        self.cache_read.saturating_add(self.cache_write)
    }

    /// 模型生成的全部 token（`output + reasoning`），用于输出计价与速度计算。
    pub fn output_with_reasoning(&self) -> u64 {
        self.output.saturating_add(self.reasoning)
    }
}

/// 各 adapter 统一产出的请求级归一化事件。
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    /// 产生该请求的 Agent。
    pub agent: Agent,
    /// 源里的会话标识。
    pub session: String,
    /// 可选的人类可读会话标签（如 dsh 的项目目录名、opencode 的会话标题）。
    pub session_label: Option<String>,
    /// 源里记录的 provider / 网关名（分组时缺省归为 `unknown`）。
    pub provider: Option<String>,
    /// 统一后的小写模型名（未知模型保留小写原名）。
    pub model: String,
    /// 事件时间戳（毫秒，按本地时区分桶）。
    pub ts_ms: i64,
    /// 本次请求的 token 分项。
    pub usage: Usage,
    /// 入口按官方美元价结算出的费用（未知模型为 `None`）。
    pub cost_usd: Option<f64>,
    /// 入口结算所用的价目快照（未知模型为 `None`，下游只读不二次查表）。
    pub pricing: Option<ModelPricing>,
    /// 首 token 延迟（毫秒），源无可靠样本时为 `None`。
    pub ttft_ms: Option<u64>,
    /// 纯解码窗口（毫秒，剔除 TTFT 与工具执行），源无可靠样本时为 `None`。
    pub duration_ms: Option<u64>,
    /// 可选执行体角色（如主代理 / 子代理）。
    pub role: Option<String>,
    /// 可选 turn 标识。
    pub turn: Option<String>,
    /// 请求是否因错误或中止而结束。
    pub aborted: bool,
}
