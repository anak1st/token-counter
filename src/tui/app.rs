//! TUI 应用状态与按键交互处理。
//!
//! 当前阶段聚焦两个视图：`Overview`（纵览）与 `Day`（按天明细联动），
//! 所有列表严格按时间倒序排列（最新在最上面），不引入排序切换操作。

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use super::data::{ModelRow, TuiSnapshot};

/// 后台自动刷新周期（60 秒）。
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// 当前支持的 TUI Tab 集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Day,
}

impl Tab {
    /// 按展示顺序排列的全部 Tab。
    pub const ALL: [Tab; 2] = [Tab::Overview, Tab::Day];

    /// Tab 栏英文标题。
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Day => "Day",
        }
    }

    /// 切换到下一个 Tab（循环）。
    pub fn next(self) -> Self {
        match self {
            Tab::Overview => Tab::Day,
            Tab::Day => Tab::Overview,
        }
    }

    /// 切换到上一个 Tab（循环）。
    pub fn prev(self) -> Self {
        match self {
            Tab::Overview => Tab::Day,
            Tab::Day => Tab::Overview,
        }
    }
}

/// 按键处理后向主事件循环返回的动作意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    None,
    RefreshNow,
    Quit,
}

/// TUI 顶层运行状态。
pub struct App {
    pub current_tab: Tab,
    pub snapshot: Option<TuiSnapshot>,
    pub refreshing: bool,
    pub last_error: Option<String>,
    pub last_refresh_at: Option<Instant>,
    pub spinner_tick: usize,
    pub overview_table: TableState,
    pub day_table: TableState,
}

impl App {
    /// 创建初始状态（启动即进入后台加载）。
    pub fn new() -> Self {
        let mut overview_table = TableState::default();
        overview_table.select(Some(0));

        let mut day_table = TableState::default();
        day_table.select(Some(0));

        Self {
            current_tab: Tab::Overview,
            snapshot: None,
            refreshing: true,
            last_error: None,
            last_refresh_at: None,
            spinner_tick: 0,
            overview_table,
            day_table,
        }
    }

    /// 推进加载动画帧计数器。
    pub fn tick(&mut self) {
        if self.refreshing {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
    }

    /// 距离下次 60s 自动刷新的剩余秒数。
    pub fn secs_until_next_refresh(&self) -> u64 {
        let last = match self.last_refresh_at {
            Some(t) => t,
            None => return REFRESH_INTERVAL.as_secs(),
        };
        let elapsed = last.elapsed();
        REFRESH_INTERVAL.saturating_sub(elapsed).as_secs()
    }

    /// 应用后台线程推送的新快照，并保持当前滚动选中行在合法范围内。
    pub fn apply_snapshot(&mut self, snapshot: TuiSnapshot) {
        let overview_len = snapshot.model_rows.len();
        let day_len = snapshot.daily_rows.len();

        self.snapshot = Some(snapshot);
        self.refreshing = false;
        self.last_error = None;
        self.last_refresh_at = Some(Instant::now());

        clamp_selection(&mut self.overview_table, overview_len);
        clamp_selection(&mut self.day_table, day_len);
    }

    /// 记录后台刷新错误（保留上一次成功快照不清空屏幕）。
    pub fn apply_error(&mut self, error: String) {
        self.refreshing = false;
        self.last_error = Some(error);
        self.last_refresh_at = Some(Instant::now());
    }

    /// 返回 `Day` Tab 当前选中日期对应的 `(日期字符串, 当日明细列表)`。
    pub fn selected_day_breakdown(&self) -> Option<(&str, &[ModelRow])> {
        let snap = self.snapshot.as_ref()?;
        let idx = self.day_table.selected()?;
        let row = snap.daily_rows.get(idx)?;
        let breakdown = snap
            .daily_breakdowns
            .get(&row.label)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        Some((row.label.as_str(), breakdown))
    }

    /// 处理单次键盘事件。
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return KeyAction::Quit;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => KeyAction::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.current_tab = self.current_tab.next();
                KeyAction::None
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.current_tab = self.current_tab.prev();
                KeyAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll(-1);
                KeyAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll(1);
                KeyAction::None
            }
            KeyCode::PageUp => {
                self.scroll(-10);
                KeyAction::None
            }
            KeyCode::PageDown => {
                self.scroll(10);
                KeyAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.jump_to(0);
                KeyAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                let len = self.active_list_len();
                if len > 0 {
                    self.jump_to(len - 1);
                }
                KeyAction::None
            }
            KeyCode::Char('r') => {
                if !self.refreshing {
                    self.refreshing = true;
                    return KeyAction::RefreshNow;
                }
                KeyAction::None
            }
            _ => KeyAction::None,
        }
    }

    /// 返回当前激活 Tab 主滚动列表的行数。
    fn active_list_len(&self) -> usize {
        let snap = match self.snapshot.as_ref() {
            Some(s) => s,
            None => return 0,
        };
        match self.current_tab {
            Tab::Overview => snap.model_rows.len(),
            Tab::Day => snap.daily_rows.len(),
        }
    }

    /// 按偏移量滚动当前 Tab 的主表选中行。
    fn scroll(&mut self, delta: isize) {
        let len = self.active_list_len();
        if len == 0 {
            return;
        }
        let state = self.active_table_mut();
        let current = state.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, (len - 1) as isize) as usize;
        state.select(Some(next));
    }

    /// 跳转到当前 Tab 主表的指定行。
    fn jump_to(&mut self, index: usize) {
        let len = self.active_list_len();
        if len == 0 {
            return;
        }
        let clamped = index.min(len - 1);
        self.active_table_mut().select(Some(clamped));
    }

    fn active_table_mut(&mut self) -> &mut TableState {
        match self.current_tab {
            Tab::Overview => &mut self.overview_table,
            Tab::Day => &mut self.day_table,
        }
    }
}

/// 将表格选中索引限制在 `0..len` 内（列表为空时取消选中）。
fn clamp_selection(state: &mut TableState, len: usize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let idx = state.selected().unwrap_or(0).min(len - 1);
    state.select(Some(idx));
}
