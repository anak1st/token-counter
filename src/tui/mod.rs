//! 交互式 TUI 入口：终端初始化/恢复、主渲染事件循环与 60s 后台自动刷新线程。

mod app;
mod data;
mod ui;

use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use token_counter::core::pricing::PricingCatalog;

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

    // 启动后台数据加载线程：立即拉取首帧，随后每 60s 自动刷新一次
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

/// 后台 60s 定时扫描循环。
fn background_worker(
    catalog: PricingCatalog,
    cmd_rx: mpsc::Receiver<WorkerCmd>,
    msg_tx: mpsc::Sender<WorkerMsg>,
) {
    loop {
        let _ = msg_tx.send(WorkerMsg::Refreshing);
        match data::load_snapshot(&catalog) {
            Ok(snapshot) => {
                if msg_tx.send(WorkerMsg::Ready(snapshot)).is_err() {
                    break;
                }
            }
            Err(err) => {
                if msg_tx.send(WorkerMsg::Error(err.to_string())).is_err() {
                    break;
                }
            }
        }

        match cmd_rx.recv_timeout(REFRESH_INTERVAL) {
            Ok(WorkerCmd::RefreshNow) | Err(RecvTimeoutError::Timeout) => continue,
            Ok(WorkerCmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
