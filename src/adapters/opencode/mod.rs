//! OpenCode 只读 SQLite adapter。

mod decode;
mod scan;

pub use scan::{default_db_path, scan, scan_path};
