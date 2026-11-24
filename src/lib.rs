//! token-counter 核心库入口。
//!
//! 按职责拆分为三层：`core`（领域基础）、`adapters`（本地数据源只读采集）、`report`（聚合与纯文本输出），
//! 与可执行入口层的 `cli` 和 `tui` 解耦。

pub mod adapters;
pub mod core;
pub mod report;
