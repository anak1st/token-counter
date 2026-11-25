//! DeepSeek Harness（dsh）v4 zstd JSONL adapter。
//!
//! 拆分为三层，职责互不交叉：
//! - `decode`：字节预过滤与强类型 JSONL 行解码；
//! - `parser`：单会话纯状态机（路由、step 起点、fork 去重与 turn 中止回填）；
//! - `scan`：候选文件剪枝与多核并行解压调度。

mod decode;
mod parser;
mod scan;

pub use scan::{default_sessions_dir, scan, scan_dir, scan_file};
