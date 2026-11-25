//! 交互式 TUI 入口：终端初始化/恢复、主渲染事件循环与梯度 60s 后台自动刷新线程。
//!
//! 加载与刷新策略（零磁盘落盘）：
//! 1. 启动先扫最近 30 天 `[initial_cutoff_ms, ∞)`，快速点亮首屏与 30 天柱图；
//! 2. 后台静默补扫更早历史 `[0, initial_cutoff_ms)` 并合并推出全量快照，
//!    随后将今日 00:00 之前的所有事件冻结驻留内存；
//! 3. 之后每 60s（或按 `r`）仅重扫今日 `[refresh_cutoff_ms, ∞)` 再拼合冻结历史，
//!    既快又不重计不漏计。

use std::io;
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use ratatui::{self, DefaultTerminal};
use token_counter::core::model::UsageEvent;
use token_counter::core::pricing::PricingCatalog;
use token_counter::core::time::TimeRange;

use super::app::{App, KeyAction, REFRESH_INTERVAL};
use super::data::{self, TuiSnapshot, INITIAL_WINDOW_DAYS, REFRESH_WINDOW_DAYS};
use super::ui;

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
    Ready(Box<TuiSnapshot>),
    Error(String),
}

/// 启动全屏 Ratatui TUI；无论正常退出还是出错均保证关闭鼠标捕获并恢复终端状态。
pub fn run(catalog: PricingCatalog) -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let result = run_event_loop(&mut terminal, catalog);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// 主事件与渲染循环。
fn run_event_loop(terminal: &mut DefaultTerminal, catalog: PricingCatalog) -> Result<()> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let (msg_tx, msg_rx) = mpsc::channel::<WorkerMsg>();

    // 启动后台两段式加载与 60s 定时刷新线程
    thread::Builder::new()
        .name("tc-tui-loader".to_string())
        .spawn(move || background_worker(catalog, cmd_rx, msg_tx))?;

    let mut app = App::new();

    loop {
        // 消费后台线程回传的刷新状态或新快照（非阻塞）
        loop {
            match msg_rx.try_recv() {
                Ok(WorkerMsg::Refreshing) => {
                    app.refreshing = true;
                }
                Ok(WorkerMsg::Ready(snapshot)) => {
                    app.apply_snapshot(*snapshot);
                }
                Ok(WorkerMsg::Error(err)) => {
                    app.apply_error(err);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }

        terminal.draw(|frame| ui::render(frame, &mut app))?;

        // 非阻塞轮询键盘与鼠标事件，超时则推进动画帧并重绘倒计时
        if event::poll(POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match app.handle_key(key) {
                    KeyAction::Quit => break,
                    KeyAction::RefreshNow => {
                        let _ = cmd_tx.send(WorkerCmd::RefreshNow);
                    }
                    KeyAction::None => {}
                },
                Event::Mouse(mouse) => {
                    app.handle_mouse(mouse);
                }
                _ => {}
            }
        }

        app.tick();
    }

    // 通知后台线程温和退出，不阻塞主线程关闭界面
    let _ = cmd_tx.send(WorkerCmd::Stop);
    Ok(())
}

/// 后台梯度加载（30 天首屏 + 历史补全）与 60s 当日增量刷新循环。
fn background_worker(
    catalog: PricingCatalog,
    cmd_rx: mpsc::Receiver<WorkerCmd>,
    msg_tx: mpsc::Sender<WorkerMsg>,
) {
    let initial_cutoff_ms = match data::window_cutoff_ms(INITIAL_WINDOW_DAYS) {
        Ok(ms) => ms,
        Err(err) => {
            let _ = msg_tx.send(WorkerMsg::Error(err.to_string()));
            return;
        }
    };
    let refresh_cutoff_ms = match data::window_cutoff_ms(REFRESH_WINDOW_DAYS) {
        Ok(ms) => ms,
        Err(err) => {
            let _ = msg_tx.send(WorkerMsg::Error(err.to_string()));
            return;
        }
    };

    let initial_range = TimeRange {
        start_ms: initial_cutoff_ms,
        end_ms: i64::MAX,
    };
    let older_range = TimeRange {
        start_ms: 0,
        end_ms: initial_cutoff_ms,
    };
    let refresh_range = TimeRange {
        start_ms: refresh_cutoff_ms,
        end_ms: i64::MAX,
    };

    // 阶段 1：先扫最近 30 天，快速点亮首屏与 30 天堆叠柱状图
    let _ = msg_tx.send(WorkerMsg::Refreshing);
    let init_started = Instant::now();
    let recent_events: Vec<UsageEvent> = match data::scan_range(initial_range, &catalog) {
        Ok(events) => events,
        Err(err) => {
            let _ = msg_tx.send(WorkerMsg::Error(err.to_string()));
            Vec::new()
        }
    };
    let partial_snap = data::build_snapshot(
        &recent_events,
        true,
        init_started.elapsed().as_millis() as u64,
    );
    if msg_tx.send(WorkerMsg::Ready(Box::new(partial_snap))).is_err() {
        return;
    }

    // 若用户在首屏阶段已退出，直接结束不再补扫历史
    if matches!(
        cmd_rx.try_recv(),
        Ok(WorkerCmd::Stop) | Err(TryRecvError::Disconnected)
    ) {
        return;
    }

    // 阶段 2：后台静默补扫 30 天前的历史，合并后推送全量快照
    let older_started = Instant::now();
    let older_events: Vec<UsageEvent> = match data::scan_range(older_range, &catalog) {
        Ok(events) => events,
        Err(err) => {
            let _ = msg_tx.send(WorkerMsg::Error(err.to_string()));
            Vec::new()
        }
    };
    let full_snap = build_merged_snapshot(
        &older_events,
        &recent_events,
        older_started.elapsed().as_millis() as u64,
    );
    if msg_tx.send(WorkerMsg::Ready(Box::new(full_snap))).is_err() {
        return;
    }

    // 将今日 00:00 之前的所有历史事件冻结驻留内存；后续刷新仅重扫今日 `[refresh_cutoff_ms, ∞)`
    let mut frozen_events = older_events;
    for event in recent_events {
        if event.ts_ms < refresh_cutoff_ms {
            frozen_events.push(event);
        }
    }

    // 阶段 3：后续每 60s（或手动 `r`）仅重扫今天（1 天）再与冻结历史拼合
    while let Ok(WorkerCmd::RefreshNow) | Err(RecvTimeoutError::Timeout) =
        cmd_rx.recv_timeout(REFRESH_INTERVAL)
    {
        let _ = msg_tx.send(WorkerMsg::Refreshing);
        let started = Instant::now();
        match data::scan_range(refresh_range, &catalog) {
            Ok(today_events) => {
                let snap = build_merged_snapshot(
                    &frozen_events,
                    &today_events,
                    started.elapsed().as_millis() as u64,
                );
                if msg_tx.send(WorkerMsg::Ready(Box::new(snap))).is_err() {
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

/// 合并内存中的冻结历史事件与最新增量窗口事件，生成完整快照。
fn build_merged_snapshot(
    frozen_events: &[UsageEvent],
    fresh_events: &[UsageEvent],
    load_ms: u64,
) -> TuiSnapshot {
    let mut combined = Vec::with_capacity(frozen_events.len() + fresh_events.len());
    combined.extend_from_slice(frozen_events);
    combined.extend_from_slice(fresh_events);
    data::build_snapshot(&combined, false, load_ms)
}
