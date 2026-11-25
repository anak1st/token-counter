//! 命令行参数解析、单次报表执行与纯文本渲染模块。

mod args;
mod report_render;
mod runner;
mod session_render;
mod spinner;

pub use args::{Cli, Command};
pub use runner::run;
