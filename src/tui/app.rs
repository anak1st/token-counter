//! TUI 应用状态与按键交互处理。
//!
//! 支持 `Overview`、`Hour`、`Day`、`Week`、`Month`、`Session` 六个视图：
//! - 所有时间表与会话列表严格按时间倒序排列（最新在最上面），不做排序操作；
//! - 每个 Tab（以及 `Session` 下各 Agent）独立维护滚动选中状态，切换不丢失位置。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use token_counter::core::model::Agent;
use token_counter::report::session::SessionDetail;

use super::data::{ModelRow, PeriodData, TuiSnapshot};

/// 后台自动刷新周期（60 秒）。
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// 当前支持的 TUI Tab 集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Hour,
    Day,
    Week,
    Month,
    Session,
}

impl Tab {
    /// 按展示顺序排列的全部 Tab。
    pub const ALL: [Tab; 6] = [
        Tab::Overview,
        Tab::Hour,
        Tab::Day,
        Tab::Week,
        Tab::Month,
        Tab::Session,
    ];

    /// Tab 栏英文标题。
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Hour => "Hour",
            Tab::Day => "Day",
            Tab::Week => "Week",
            Tab::Month => "Month",
            Tab::Session => "Session",
        }
    }

    /// 切换到下一个 Tab（循环）。
    pub fn next(self) -> Self {
        match self {
            Tab::Overview => Tab::Hour,
            Tab::Hour => Tab::Day,
            Tab::Day => Tab::Week,
            Tab::Week => Tab::Month,
            Tab::Month => Tab::Session,
            Tab::Session => Tab::Overview,
        }
    }

    /// 切换到上一个 Tab（循环）。
    pub fn prev(self) -> Self {
        match self {
            Tab::Overview => Tab::Session,
            Tab::Hour => Tab::Overview,
            Tab::Day => Tab::Hour,
            Tab::Week => Tab::Day,
            Tab::Month => Tab::Week,
            Tab::Session => Tab::Month,
        }
    }
}

/// `Session` Tab 当前的键盘滚动焦点（上半区会话列表 vs 下半区轮次明细）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFocus {
    Sessions,
    Turns,
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
    pub hour_table: TableState,
    pub day_table: TableState,
    pub week_table: TableState,
    pub month_table: TableState,
    pub session_agent: Agent,
    pub session_focus: SessionFocus,
    pub session_tables: HashMap<Agent, TableState>,
    pub turn_table: TableState,
}

impl App {
    /// 创建初始状态（启动即进入后台两段式加载）。
    pub fn new() -> Self {
        let mut session_tables = HashMap::with_capacity(Agent::ALL.len());
        for agent in Agent::ALL {
            session_tables.insert(agent, initial_table_state());
        }

        Self {
            current_tab: Tab::Overview,
            snapshot: None,
            refreshing: true,
            last_error: None,
            last_refresh_at: None,
            spinner_tick: 0,
            overview_table: initial_table_state(),
            hour_table: initial_table_state(),
            day_table: initial_table_state(),
            week_table: initial_table_state(),
            month_table: initial_table_state(),
            session_agent: Agent::ALL[0],
            session_focus: SessionFocus::Sessions,
            session_tables,
            turn_table: initial_table_state(),
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

    /// 应用后台线程推送的新快照，并保持各 Tab 选中行在合法范围内。
    pub fn apply_snapshot(&mut self, snapshot: TuiSnapshot) {
        let overview_len = snapshot.model_rows.len();
        let hour_len = snapshot.hourly.rows.len();
        let day_len = snapshot.daily.rows.len();
        let week_len = snapshot.weekly.rows.len();
        let month_len = snapshot.monthly.rows.len();
        let still_backfilling = snapshot.is_partial;

        let mut agent_lens: Vec<(Agent, usize)> = Vec::with_capacity(Agent::ALL.len());
        for agent in Agent::ALL {
            let count = snapshot
                .sessions_by_agent
                .get(&agent)
                .map(Vec::len)
                .unwrap_or(0);
            agent_lens.push((agent, count));
        }

        self.snapshot = Some(snapshot);
        self.refreshing = still_backfilling;
        self.last_error = None;
        if !still_backfilling {
            self.last_refresh_at = Some(Instant::now());
        }

        clamp_selection(&mut self.overview_table, overview_len);
        clamp_selection(&mut self.hour_table, hour_len);
        clamp_selection(&mut self.day_table, day_len);
        clamp_selection(&mut self.week_table, week_len);
        clamp_selection(&mut self.month_table, month_len);

        for (agent, count) in agent_lens {
            if let Some(table) = self.session_tables.get_mut(&agent) {
                clamp_selection(table, count);
            }
        }

        self.clamp_turn_selection();
    }

    /// 记录后台刷新错误（保留上一次成功快照不清空屏幕）。
    pub fn apply_error(&mut self, error: String) {
        self.refreshing = false;
        self.last_error = Some(error);
        self.last_refresh_at = Some(Instant::now());
    }

    /// 返回指定时间维度当前选中分桶的 `(分桶标签, 当期模型明细列表)`。
    pub fn selected_period_breakdown<'a>(
        period: &'a PeriodData,
        table: &TableState,
    ) -> Option<(&'a str, &'a [ModelRow])> {
        let idx = table.selected()?;
        let row = period.rows.get(idx)?;
        let breakdown = period
            .breakdowns
            .get(&row.label)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        Some((row.label.as_str(), breakdown))
    }

    /// 返回指定 Agent 下的全部会话明细切片。
    pub fn sessions_for_agent(&self, agent: Agent) -> &[SessionDetail] {
        let snap = match self.snapshot.as_ref() {
            Some(s) => s,
            None => return &[],
        };
        snap.sessions_by_agent
            .get(&agent)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// 返回当前 `Session` Tab 选中的会话详情。
    pub fn selected_session(&self) -> Option<&SessionDetail> {
        let list = self.sessions_for_agent(self.session_agent);
        let idx = self
            .session_tables
            .get(&self.session_agent)
            .and_then(TableState::selected)?;
        list.get(idx)
    }

    /// 切换 `Session` Tab 当前展示的 Agent，并将焦点复位到会话列表。
    pub fn switch_session_agent(&mut self, agent: Agent) {
        if self.session_agent == agent {
            return;
        }
        self.session_agent = agent;
        self.session_focus = SessionFocus::Sessions;
        self.reset_turn_selection();
    }

    /// 循环切换 `Session` Tab 的 Agent（`forward = true` 向右，`false` 向左）。
    pub fn cycle_session_agent(&mut self, forward: bool) {
        let total = Agent::ALL.len();
        if total == 0 {
            return;
        }
        let current_idx = Agent::ALL
            .iter()
            .position(|&a| a == self.session_agent)
            .unwrap_or(0);
        let next_idx = if forward {
            (current_idx + 1) % total
        } else {
            (current_idx + total - 1) % total
        };
        self.switch_session_agent(Agent::ALL[next_idx]);
    }

    /// 处理单次键盘事件。
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return KeyAction::Quit;
        }

        // `Session` Tab 专属交互：Agent 按钮切换、Enter 切焦点、Esc 返回上表
        if self.current_tab == Tab::Session {
            match key.code {
                KeyCode::Char('a') => {
                    self.cycle_session_agent(true);
                    return KeyAction::None;
                }
                KeyCode::Char('A') => {
                    self.cycle_session_agent(false);
                    return KeyAction::None;
                }
                KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                    let idx = (c as usize) - ('1' as usize);
                    if idx < Agent::ALL.len() {
                        self.switch_session_agent(Agent::ALL[idx]);
                    }
                    return KeyAction::None;
                }
                KeyCode::Enter => {
                    self.toggle_session_focus();
                    return KeyAction::None;
                }
                KeyCode::Esc if self.session_focus == SessionFocus::Turns => {
                    self.session_focus = SessionFocus::Sessions;
                    return KeyAction::None;
                }
                _ => {}
            }
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

    /// 在 `Session` Tab 的会话列表与 Turn 列表之间切换键盘滚动焦点。
    fn toggle_session_focus(&mut self) {
        match self.session_focus {
            SessionFocus::Sessions => {
                let turn_count = self
                    .selected_session()
                    .map(|s| s.turns.len())
                    .unwrap_or(0);
                if turn_count > 0 {
                    self.session_focus = SessionFocus::Turns;
                    self.clamp_turn_selection();
                }
            }
            SessionFocus::Turns => {
                self.session_focus = SessionFocus::Sessions;
            }
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
            Tab::Hour => snap.hourly.rows.len(),
            Tab::Day => snap.daily.rows.len(),
            Tab::Week => snap.weekly.rows.len(),
            Tab::Month => snap.monthly.rows.len(),
            Tab::Session => match self.session_focus {
                SessionFocus::Sessions => self.sessions_for_agent(self.session_agent).len(),
                SessionFocus::Turns => self
                    .selected_session()
                    .map(|s| s.turns.len())
                    .unwrap_or(0),
            },
        }
    }

    /// 按偏移量滚动当前聚焦表格的选中行。
    fn scroll(&mut self, delta: isize) {
        let len = self.active_list_len();
        if len == 0 {
            return;
        }
        let prev_selected = self.active_table_mut().selected();
        let current = prev_selected.unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, (len - 1) as isize) as usize;
        self.active_table_mut().select(Some(next));

        if self.current_tab == Tab::Session
            && self.session_focus == SessionFocus::Sessions
            && prev_selected != Some(next)
        {
            self.reset_turn_selection();
        }
    }

    /// 跳转到当前聚焦表格的指定行。
    fn jump_to(&mut self, index: usize) {
        let len = self.active_list_len();
        if len == 0 {
            return;
        }
        let clamped = index.min(len - 1);
        let prev_selected = self.active_table_mut().selected();
        self.active_table_mut().select(Some(clamped));

        if self.current_tab == Tab::Session
            && self.session_focus == SessionFocus::Sessions
            && prev_selected != Some(clamped)
        {
            self.reset_turn_selection();
        }
    }

    fn active_table_mut(&mut self) -> &mut TableState {
        match self.current_tab {
            Tab::Overview => &mut self.overview_table,
            Tab::Hour => &mut self.hour_table,
            Tab::Day => &mut self.day_table,
            Tab::Week => &mut self.week_table,
            Tab::Month => &mut self.month_table,
            Tab::Session => match self.session_focus {
                SessionFocus::Sessions => self
                    .session_tables
                    .entry(self.session_agent)
                    .or_insert_with(initial_table_state),
                SessionFocus::Turns => &mut self.turn_table,
            },
        }
    }

    /// 将 Turn 列表选中行复位到第一条（切换 Agent 或切换选中 Session 时调用）。
    fn reset_turn_selection(&mut self) {
        let turn_len = self
            .selected_session()
            .map(|s| s.turns.len())
            .unwrap_or(0);
        self.turn_table = TableState::default();
        if turn_len > 0 {
            self.turn_table.select(Some(0));
        }
    }

    /// 将 Turn 列表选中行裁剪到当前选中会话的合法范围内。
    fn clamp_turn_selection(&mut self) {
        let turn_len = self
            .selected_session()
            .map(|s| s.turns.len())
            .unwrap_or(0);
        if turn_len == 0 {
            self.session_focus = SessionFocus::Sessions;
        }
        clamp_selection(&mut self.turn_table, turn_len);
    }
}

fn initial_table_state() -> TableState {
    let mut state = TableState::default();
    state.select(Some(0));
    state
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
