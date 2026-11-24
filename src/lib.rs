//! token-counter 核心库入口。
//!
//! 将领域模型、价目表、时间分桶、数据源 adapter、聚合、会话分析与纯文本渲染与 CLI/TUI 入口解耦。

pub mod adapters;
pub mod aggregate;
pub mod model;
pub mod pricing;
pub mod render;
pub mod session;
pub mod time;
