//! Antigravity（agy）只读 SQLite + protobuf adapter。

mod proto;
mod scan;

pub use scan::{default_conversations_dir, scan, scan_conversation, scan_dir};
