//! 交互式 TUI 入口：终端初始化/恢复、主渲染事件循环与两段式 60s 后台自动刷新线程。
//!
//! 两段式梯度加载策略（零磁盘落盘）：
//! 1. 启动先扫最近 30 天热窗 `[cutoff_ms, ∞)`，毫秒级推送首屏；
//! 2. 紧接着后台补扫更早冷窗 `[0, cutoff_ms)` 并常驻线程内存，合并推送完整历史；
//! 3. 之后每 60s（或按 `r`）只重扫热窗再拼合内存冷窗，既快又不重计不漏计。

mod app;
mod data;
mod ui;

use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use token_counter::core::model::UsageEvent;
use token_counter::core::pricing::PricingCatalog;
use token_counter::core::time::TimeRange;

use self::app::{App, KeyAction, REFRESH_INTERVAL};
use self::data::TuiSnapshot;

/// 事件循环轮询间隔（80ms，兼顾按键跟手与动画流畅且低 CPU 占用）。
const POLL_INTERVAL: Duration = Duration::from_millis(80);

/// 主线程发给后台加载线程的控制命令。
enum WorkerCmd {
    RefreshNow,
    Stop,
}

/// 后台加载线程回传给主线程的状态消息。
enum WorkerMsg {
    Refreshing,
    Ready(TuiSnapshot),
    Error(String),
}

/// 启动全屏 Ratatui TUI；无论正常退出还是出错均保证恢复终端状态。
pub fn run(catalog: PricingCatalog) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_event_loop(&mut terminal, catalog);
    ratatui::restore();
    result
}

/// 主事件与渲染循环。
fn run_event_loop(terminal: &mut ratatui::DefaultTerminal, catalog: PricingCatalog) -> Result<()> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let (msg_tx, msg_rx) = mpsc::channel::<WorkerMsg>();

    // 启动后台两段式加载与 60s 定时刷新线程
    let _worker = thread::Builder::new()
        .name("tc-tui-loader".to_string())
        .spawn(move || background_worker(catalog, cmd_rx, msg_tx));

    let mut app = App::new();

    loop {
        // 消费后台线程回传的刷新状态或新快照（非阻塞）
        loop {
            match msg_rx.try_recv() {
                Ok(WorkerMsg::Refreshing) => {
                    app.refreshing = true;
                }
                Ok(WorkerMsg::Ready(snapshot)) => {
                    app.apply_snapshot(snapshot);
                }
                Ok(WorkerMsg::Error(err)) => {
                    app.apply_error(err);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }

        terminal.draw(|frame| ui::render(frame, &mut app))?;

        // 非阻塞轮询键盘事件，超时则推进动画帧并重绘倒计时
        if event::poll(POLL_INTERVAL)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match app.handle_key(key) {
                        KeyAction::Quit => break,
                        KeyAction::RefreshNow => {
                            let _ = cmd_tx.send(WorkerCmd::RefreshNow);
                        }
                        KeyAction::None => {}
                    }
                }
            }
        }

        app.tick();
    }

    // 通知后台线程温和退出，不阻塞主线程关闭界面
    let _ = cmd_tx.send(WorkerCmd::Stop);
    Ok(())
}

/// 后台两段式加载 + 60s 热窗定时刷新循环。
fn background_worker(
    catalog: PricingCatalog,
    cmd_rx: mpsc::Receiver<WorkerCmd>,
    msg_tx: mpsc::Sender<WorkerMsg>,
) {
    let cutoff_ms = match data::hot_window_cutoff_ms() {
        Ok(ms) => ms,
        Err(err) => {
            let _ = msg_tx.send(WorkerMsg::Error(err.to_string()));
            return;
        }
    };

    let hot_range = TimeRange {
        start_ms: cutoff_ms,
        end_ms: i64::MAX,
    };
    let cold_range = TimeRange {
        start_ms: 0,
        end_ms: cutoff_ms,
    };

    // 阶段 1：先扫最近 30 天热窗，快速点亮首屏
    let _ = msg_tx.send(WorkerMsg::Refreshing);
    let hot_started = Instant::now();
    let mut hot_events: Vec<UsageEvent> = match data::scan_range(hot_range, &catalog) {
        Ok(events) => events,
        Err(err) => {
            let _ = msg_tx.send(WorkerMsg::Error(err.to_string()));
            Vec::new()
        }
    };
    let partial_snap = data::build_snapshot(&hot_events, true, hot_started.elapsed().as_millis() as u64);
    if msg_tx.send(WorkerMsg::Ready(partial_snap)).is_err() {
        return;
    }

    // 若用户在首屏阶段已退出，直接结束不再补扫历史
    if matches!(cmd_rx.try_recv(), Ok(WorkerCmd::Stop) | Err(TryRecvError::Disconnected)) {
        return;
    }

    // 阶段 2：后台静默补扫 30 天前的历史冷窗（只扫一次，驻留内存）
    let cold_started = Instant::now();
    let cold_events: Vec<UsageEvent> = match data::scan_range(cold_range, &catalog) {
        Ok(events) => events,
        Err(err) => {
            let _ = msg_tx.send(WorkerMsg::Error(err.to_string()));
            Vec::new()
        }
    };
    let full_snap = build_merged_snapshot(
        &cold_events,
        &hot_events,
        cold_started.elapsed().as_millis() as u64,
    );
    if msg_tx.send(WorkerMsg::Ready(full_snap)).is_err() {
        return;
    }

    // 阶段 3：后续每 60s（或手动 `r`）只重扫热窗并与内存冷窗合并
    loop {
        match cmd_rx.recv_timeout(REFRESH_INTERVAL) {
            Ok(WorkerCmd::RefreshNow) | Err(RecvTimeoutError::Timeout) => {}
            Ok(WorkerCmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }

        let _ = msg_tx.send(WorkerMsg::Refreshing);
        let started = Instant::now();
        match data::scan_range(hot_range, &catalog) {
            Ok(fresh_hot) => {
                hot_events = fresh_hot;
                let snap = build_merged_snapshot(
                    &cold_events,
                    &hot_events,
                    started.elapsed().as_millis() as u64,
                );
                if msg_tx.send(WorkerMsg::Ready(snap)).is_err() {
                    break;
                }
            }
            Err(err) => {
                if msg_tx.send(WorkerMsg::Error(err.to_string())).is_err() {
                    break;
                }
            }
        }
    }
}

/// 合并内存中的历史冷窗事件与最新热窗事件，生成完整快照。
fn build_merged_snapshot(
    cold_events: &[UsageEvent],
    hot_events: &[UsageEvent],
    load_ms: u64,
) -> TuiSnapshot {
    let mut combined = Vec::with_capacity(cold_events.len() + hot_events.len());
    combined.extend_from_slice(cold_events);
    combined.extend_from_slice(hot_events);
    data::build_snapshot(&combined, false, load_ms)
}
