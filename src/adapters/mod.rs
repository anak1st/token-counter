//! 内置三个 Agent（`opencode`、`dsh`、`agy`）的数据源 adapter 统一分发层。

pub mod agy;
pub mod dsh;
pub mod opencode;
mod scan;

pub use scan::{scan_agent, scan_agents, scan_agents_query, ScanQuery};
