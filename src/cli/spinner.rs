//! 单次 CLI 的轻量级终端加载动画（借助 `crossterm` 兼容 Unix 与 Windows 控制台）。
//!
//! 特点：
//! - 仅在 `stderr` 为交互式终端（TTY）时渲染，重定向到文件或管道时自动静默；
//! - 附带实时读秒（`0.1s` 精度），延迟首帧避免极快查询闪屏；
//! - 使用 `Condvar` 通知退出，查询结束时毫秒级唤醒，绝不拖慢快查询；
//! - 结束（或提前 `Drop`）时用终端控制清行且不换行，使后续输出直接从同一行覆盖开始。

use std::io::{self, IsTerminal};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::cursor::MoveToColumn;
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{Clear, ClearType};

/// Braille 旋转动画帧序列。
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 首帧延迟（小于该耗时直接出表，避免闪烁）。
const INITIAL_DELAY: Duration = Duration::from_millis(80);

/// 动画刷新间隔。
const TICK_INTERVAL: Duration = Duration::from_millis(80);

/// 后台动画线程的生命周期守卫；`stop()` 或 `Drop` 时保证清理当前行。
pub struct Spinner {
    state: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<bool>>,
}

impl Spinner {
    /// 启动加载动画（非 TTY 时返回空守卫，不创建后台线程）。
    pub fn start(message: &'static str) -> Self {
        if !io::stderr().is_terminal() {
            return Self {
                state: Arc::new((Mutex::new(true), Condvar::new())),
                handle: None,
            };
        }

        let state = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_state = Arc::clone(&state);

        let handle = thread::spawn(move || run_loop(worker_state, message));

        Self {
            state,
            handle: Some(handle),
        }
    }

    /// 停止动画并擦除当前行，确保后续 stdout 输出从同一行行首覆盖。
    pub fn stop(mut self) {
        self.finish();
    }

    /// 内部收敛逻辑，供 `stop` 与 `Drop` 共用。
    fn finish(&mut self) {
        let handle = match self.handle.take() {
            Some(h) => h,
            None => return,
        };

        let (lock, cvar) = &*self.state;
        if let Ok(mut stopped) = lock.lock() {
            *stopped = true;
            cvar.notify_one();
        }

        let rendered = handle.join().unwrap_or(false);
        if rendered {
            let mut stderr = io::stderr().lock();
            let _ = execute!(stderr, MoveToColumn(0), Clear(ClearType::CurrentLine));
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.finish();
    }
}

/// 后台渲染循环；返回是否至少绘制过一帧。
fn run_loop(state: Arc<(Mutex<bool>, Condvar)>, message: &'static str) -> bool {
    let (lock, cvar) = &*state;
    let started = Instant::now();
    let mut rendered = false;
    let mut frame_idx = 0usize;

    let mut stopped = match lock.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };

    // 先等待一小段时间，极快的查询直接跳过绘制
    if !*stopped {
        let (guard, _) = match cvar.wait_timeout(stopped, INITIAL_DELAY) {
            Ok(res) => res,
            Err(_) => return false,
        };
        stopped = guard;
    }

    while !*stopped {
        let elapsed = started.elapsed().as_secs_f64();
        let frame = FRAMES[frame_idx % FRAMES.len()];
        frame_idx += 1;

        {
            let mut stderr = io::stderr().lock();
            let _ = execute!(
                stderr,
                MoveToColumn(0),
                Clear(ClearType::CurrentLine),
                Print(format!("{} {} {:.1}s", frame, message, elapsed))
            );
        }
        rendered = true;

        let (guard, _) = match cvar.wait_timeout(stopped, TICK_INTERVAL) {
            Ok(res) => res,
            Err(_) => break,
        };
        stopped = guard;
    }

    rendered
}
