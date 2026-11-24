//! TUI 应用状态模型与表格滚动/裁剪逻辑（键盘与鼠标事件处理位于 `input.rs`）。
//!
//! 支持 `Overview`、`Hour`、`Day`、`Week`、`Month`、`Session` 六个视图：
//! - 全部六个视图共用同一套 Agent 筛选（支持 `All` 与任意单个 Agent）；
//! - 双表视图通过 `FocusPane` 统一管理上下框焦点与联动明细表滚动；
//! - `UiLayout` 记录每帧渲染的热区，供鼠标点击与悬停滚轮精准命中。

mod input;

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::widgets::TableState;
use token_counter::core::model::Agent;
use token_counter::report::session::SessionDetail;

use super::data::{ModelRow, PeriodData, TuiSnapshot, ViewSnapshot};

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

    /// 是否为包含「上表 + 下表」双滚动框的视图。
    pub fn has_dual_panes(self) -> bool {
        self != Tab::Overview
    }
}

/// 双表视图当前的键盘滚动焦点（上半主表 vs 下半联动子表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Upper,
    Lower,
}

/// 首页堆叠柱状图的时间粒度（仅影响柱状图，KPI 与 Models 保持全量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartMode {
    Days,
    Hours,
}

impl ChartMode {
    /// 切换 `Days` / `Hours`。
    pub fn toggle(self) -> Self {
        match self {
            ChartMode::Days => ChartMode::Hours,
            ChartMode::Hours => ChartMode::Days,
        }
    }
}

/// 当前打开的居中弹窗类型（同一时刻最多打开一个弹窗）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveModal {
    AgentPicker,
    UnknownModels,
}

/// 单个可交互表格的外框区域与数据行区域（不含表头）。
#[derive(Debug, Clone, Copy)]
pub struct TableHit {
    pub panel: Rect,
    pub rows: Option<Rect>,
}

/// 最近一帧渲染记录的交互热区（供鼠标点击与悬停滚轮精准命中）。
#[derive(Debug, Default, Clone)]
pub struct UiLayout {
    /// 顶部每个 Tab 的可点击矩形区域。
    pub tab_rects: Vec<(Tab, Rect)>,
    /// 顶部框内 `Agent: ... [a]` 按钮区域。
    pub agent_button_rect: Option<Rect>,
    /// 首页柱状图内 `Days` 与 `Hours` 切换按钮区域。
    pub chart_days_rect: Option<Rect>,
    pub chart_hours_rect: Option<Rect>,
    /// 底部状态栏 `N unknown [u]` 可点击徽章区域。
    pub unknown_badge_rect: Option<Rect>,
    /// 上半主表热区（`Hour` / `Day` / `Week` / `Month` / `Session`）。
    pub upper: Option<TableHit>,
    /// 下半明细表热区（`Overview` 及各双表视图的底部面板）。
    pub lower: Option<TableHit>,
    /// 当前弹窗热区（仅弹窗打开时有效）。
    pub modal: Option<TableHit>,
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
    /// 全部 6 个视图共用的 Agent 筛选（`None` 表示 `All`）。
    pub agent_filter: Option<Agent>,
    /// 首页柱状图时间粒度（默认按天，可切按小时）。
    pub chart_mode: ChartMode,
    /// 当前打开的居中弹窗（`None` 表示无弹窗）。
    pub active_modal: Option<ActiveModal>,
    /// 当前弹窗内的滚动选中状态。
    pub modal_table: TableState,
    /// 双表视图当前聚焦的面板（上表或下表）。
    pub focus: FocusPane,
    pub overview_table: TableState,
    pub hour_table: TableState,
    pub day_table: TableState,
    pub week_table: TableState,
    pub month_table: TableState,
    pub session_table: TableState,
    /// 双表视图下半联动明细表（`Models` 或 `Turns`）共用的滚动状态。
    pub detail_table: TableState,
    /// 最近一帧的 UI 热区，供鼠标命中测试使用。
    pub layout: UiLayout,
}

impl App {
    /// 创建初始状态（启动即进入后台两段式加载）。
    pub fn new() -> Self {
        Self {
            current_tab: Tab::Overview,
            snapshot: None,
            refreshing: true,
            last_error: None,
            last_refresh_at: None,
            spinner_tick: 0,
            agent_filter: None,
            chart_mode: ChartMode::Days,
            active_modal: None,
            modal_table: initial_table_state(),
            focus: FocusPane::Upper,
            overview_table: initial_table_state(),
            hour_table: initial_table_state(),
            day_table: initial_table_state(),
            week_table: initial_table_state(),
            month_table: initial_table_state(),
            session_table: initial_table_state(),
            detail_table: initial_table_state(),
            layout: UiLayout::default(),
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

    /// 返回当前全局筛选条件生效的聚合数据视角。
    pub fn active_view(&self) -> Option<&ViewSnapshot> {
        self.snapshot.as_ref().map(|s| s.view(self.agent_filter))
    }

    /// 返回顶部按钮展示的当前 Agent 标识（`all` 或具体 agent id）。
    pub fn active_agent_label(&self) -> &'static str {
        match self.agent_filter {
            None => "all",
            Some(agent) => agent.id(),
        }
    }

    /// 返回 Agent 选择弹窗中的全部可选项（首项 `None` 表示 `All`，其后依次为所有具体 Agent）。
    pub fn picker_options(&self) -> Vec<Option<Agent>> {
        let mut options = Vec::with_capacity(Agent::ALL.len() + 1);
        options.push(None);
        for agent in Agent::ALL {
            options.push(Some(agent));
        }
        options
    }

    /// 应用后台线程推送的新快照，并保持各 Tab 选中行在合法范围内。
    pub fn apply_snapshot(&mut self, snapshot: TuiSnapshot) {
        let still_backfilling = snapshot.is_partial;

        self.snapshot = Some(snapshot);
        self.refreshing = still_backfilling;
        self.last_error = None;
        if !still_backfilling {
            self.last_refresh_at = Some(Instant::now());
        }

        self.clamp_all_tables();
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

    /// 返回当前 `Session` Tab 选中的会话详情。
    pub fn selected_session(&self) -> Option<&SessionDetail> {
        let view = self.active_view()?;
        let idx = self.session_table.selected()?;
        view.sessions.get(idx)
    }

    /// 切换顶部 Tab，并将焦点复位到上半主表与明细首行。
    pub fn switch_tab(&mut self, tab: Tab) {
        if self.current_tab == tab {
            return;
        }
        self.current_tab = tab;
        self.focus = FocusPane::Upper;
        self.reset_detail_selection();
    }

    /// 打开 Agent 选择弹窗，并默认高亮当前生效的选项。
    pub fn open_agent_picker(&mut self) {
        let options = self.picker_options();
        let selected_idx = options
            .iter()
            .position(|&opt| opt == self.agent_filter)
            .unwrap_or(0);
        self.modal_table = TableState::default();
        self.modal_table.select(Some(selected_idx));
        self.active_modal = Some(ActiveModal::AgentPicker);
    }

    /// 打开未知模型弹窗（当前视角存在未知模型时）。
    pub fn open_unknown_models(&mut self) {
        let count = self
            .active_view()
            .map(|v| v.unknown_models.len())
            .unwrap_or(0);
        if count == 0 {
            return;
        }
        self.modal_table = TableState::default();
        self.modal_table.select(Some(0));
        self.active_modal = Some(ActiveModal::UnknownModels);
    }

    /// 确认 Agent 弹窗当前选中项并应用到全部 6 个视图。
    pub fn confirm_agent_picker(&mut self) {
        let options = self.picker_options();
        let idx = self.modal_table.selected().unwrap_or(0);
        self.active_modal = None;

        let choice = match options.get(idx) {
            Some(&c) => c,
            None => return,
        };

        if self.agent_filter != choice {
            self.agent_filter = choice;
            self.focus = FocusPane::Upper;
            self.reset_all_tables();
        }
    }

    /// 返回当前打开弹窗的行数。
    pub(super) fn modal_list_len(&self) -> usize {
        match self.active_modal {
            Some(ActiveModal::AgentPicker) => self.picker_options().len(),
            Some(ActiveModal::UnknownModels) => self
                .active_view()
                .map(|v| v.unknown_models.len())
                .unwrap_or(0),
            None => 0,
        }
    }

    /// 在双表视图的上下两个框之间切换键盘滚动焦点。
    pub(super) fn toggle_focus_pane(&mut self) {
        if !self.current_tab.has_dual_panes() {
            return;
        }
        match self.focus {
            FocusPane::Upper => {
                if self.lower_list_len() > 0 {
                    self.focus = FocusPane::Lower;
                    self.clamp_detail_selection();
                }
            }
            FocusPane::Lower => {
                self.focus = FocusPane::Upper;
            }
        }
    }

    /// 当前 Tab 上半主表的行数（`Overview` 无上半表，返回 0）。
    pub(super) fn upper_list_len(&self) -> usize {
        let view = match self.active_view() {
            Some(v) => v,
            None => return 0,
        };
        match self.current_tab {
            Tab::Overview => 0,
            Tab::Hour => view.hourly.rows.len(),
            Tab::Day => view.daily.rows.len(),
            Tab::Week => view.weekly.rows.len(),
            Tab::Month => view.monthly.rows.len(),
            Tab::Session => view.sessions.len(),
        }
    }

    /// 当前 Tab 下半表（`Models` 或 `Turns`）的行数。
    pub(super) fn lower_list_len(&self) -> usize {
        let view = match self.active_view() {
            Some(v) => v,
            None => return 0,
        };
        match self.current_tab {
            Tab::Overview => view.model_rows.len(),
            Tab::Hour => Self::selected_period_breakdown(&view.hourly, &self.hour_table)
                .map(|(_, items)| items.len())
                .unwrap_or(0),
            Tab::Day => Self::selected_period_breakdown(&view.daily, &self.day_table)
                .map(|(_, items)| items.len())
                .unwrap_or(0),
            Tab::Week => Self::selected_period_breakdown(&view.weekly, &self.week_table)
                .map(|(_, items)| items.len())
                .unwrap_or(0),
            Tab::Month => Self::selected_period_breakdown(&view.monthly, &self.month_table)
                .map(|(_, items)| items.len())
                .unwrap_or(0),
            Tab::Session => self
                .selected_session()
                .map(|s| s.turns.len())
                .unwrap_or(0),
        }
    }

    /// 当前焦点表的行数。
    pub(super) fn focused_list_len(&self) -> usize {
        if !self.current_tab.has_dual_panes() || self.focus == FocusPane::Lower {
            self.lower_list_len()
        } else {
            self.upper_list_len()
        }
    }

    /// 滚动当前焦点表。
    pub(super) fn scroll_focused(&mut self, delta: isize) {
        if !self.current_tab.has_dual_panes() || self.focus == FocusPane::Lower {
            self.scroll_lower(delta);
        } else {
            self.scroll_upper(delta);
        }
    }

    /// 跳转当前焦点表到指定索引。
    pub(super) fn jump_focused(&mut self, index: usize) {
        if !self.current_tab.has_dual_panes() || self.focus == FocusPane::Lower {
            self.jump_lower(index);
        } else {
            self.jump_upper(index);
        }
    }

    /// 滚动上半主表；若选中行变化，自动将下半联动表复位到首行。
    pub(super) fn scroll_upper(&mut self, delta: isize) {
        let len = self.upper_list_len();
        if len == 0 {
            return;
        }
        let table = match self.upper_table_mut() {
            Some(t) => t,
            None => return,
        };
        let prev = table.selected();
        scroll_table(table, len, delta);
        let next = table.selected();
        if prev != next {
            self.reset_detail_selection();
        }
    }

    /// 跳转上半主表到指定行。
    fn jump_upper(&mut self, index: usize) {
        let len = self.upper_list_len();
        if len == 0 {
            return;
        }
        let clamped = index.min(len - 1);
        let table = match self.upper_table_mut() {
            Some(t) => t,
            None => return,
        };
        let prev = table.selected();
        table.select(Some(clamped));
        if prev != Some(clamped) {
            self.reset_detail_selection();
        }
    }

    /// 鼠标点击上半表第 `rel_row` 可见行。
    pub(super) fn click_upper_row(&mut self, rel_row: usize) {
        let len = self.upper_list_len();
        let table = match self.upper_table_mut() {
            Some(t) => t,
            None => return,
        };
        let target = table.offset() + rel_row;
        if target < len {
            let prev = table.selected();
            table.select(Some(target));
            if prev != Some(target) {
                self.reset_detail_selection();
            }
        }
    }

    /// 滚动下半表（`Overview` 的 `Models` 或双表视图的联动子表）。
    pub(super) fn scroll_lower(&mut self, delta: isize) {
        let len = self.lower_list_len();
        if len == 0 {
            return;
        }
        scroll_table(self.lower_table_mut(), len, delta);
    }

    /// 跳转下半表到指定行。
    fn jump_lower(&mut self, index: usize) {
        let len = self.lower_list_len();
        if len == 0 {
            return;
        }
        let clamped = index.min(len - 1);
        self.lower_table_mut().select(Some(clamped));
    }

    /// 鼠标点击下半表第 `rel_row` 可见行。
    pub(super) fn click_lower_row(&mut self, rel_row: usize) {
        let len = self.lower_list_len();
        let table = self.lower_table_mut();
        let target = table.offset() + rel_row;
        if target < len {
            table.select(Some(target));
        }
    }

    fn upper_table_mut(&mut self) -> Option<&mut TableState> {
        match self.current_tab {
            Tab::Overview => None,
            Tab::Hour => Some(&mut self.hour_table),
            Tab::Day => Some(&mut self.day_table),
            Tab::Week => Some(&mut self.week_table),
            Tab::Month => Some(&mut self.month_table),
            Tab::Session => Some(&mut self.session_table),
        }
    }

    fn lower_table_mut(&mut self) -> &mut TableState {
        if self.current_tab == Tab::Overview {
            &mut self.overview_table
        } else {
            &mut self.detail_table
        }
    }

    /// 切换全局 Agent 筛选后，将全部表格复位到首行并按新视角裁剪。
    fn reset_all_tables(&mut self) {
        self.overview_table = initial_table_state();
        self.hour_table = initial_table_state();
        self.day_table = initial_table_state();
        self.week_table = initial_table_state();
        self.month_table = initial_table_state();
        self.session_table = initial_table_state();
        self.detail_table = initial_table_state();
        self.clamp_all_tables();
    }

    /// 按当前快照裁剪全部表格（含打开中的未知模型弹窗）的选中索引。
    fn clamp_all_tables(&mut self) {
        let (overview_len, hour_len, day_len, week_len, month_len, session_len) =
            match self.active_view() {
                Some(view) => (
                    view.model_rows.len(),
                    view.hourly.rows.len(),
                    view.daily.rows.len(),
                    view.weekly.rows.len(),
                    view.monthly.rows.len(),
                    view.sessions.len(),
                ),
                None => (0, 0, 0, 0, 0, 0),
            };

        clamp_selection(&mut self.overview_table, overview_len);
        clamp_selection(&mut self.hour_table, hour_len);
        clamp_selection(&mut self.day_table, day_len);
        clamp_selection(&mut self.week_table, week_len);
        clamp_selection(&mut self.month_table, month_len);
        clamp_selection(&mut self.session_table, session_len);

        self.clamp_detail_selection();

        if self.active_modal == Some(ActiveModal::UnknownModels) {
            let unknown_len = self.modal_list_len();
            if unknown_len == 0 {
                self.active_modal = None;
            } else {
                clamp_selection(&mut self.modal_table, unknown_len);
            }
        }
    }

    /// 将下半联动明细表的选中行复位到首行。
    fn reset_detail_selection(&mut self) {
        let len = if self.current_tab.has_dual_panes() {
            self.lower_list_len()
        } else {
            0
        };
        self.detail_table = TableState::default();
        if len > 0 {
            self.detail_table.select(Some(0));
        }
    }

    /// 将下半联动明细表的选中行裁剪到合法范围内。
    fn clamp_detail_selection(&mut self) {
        if !self.current_tab.has_dual_panes() {
            return;
        }
        let len = self.lower_list_len();
        if len == 0 {
            self.focus = FocusPane::Upper;
        }
        clamp_selection(&mut self.detail_table, len);
    }
}

fn initial_table_state() -> TableState {
    let mut state = TableState::default();
    state.select(Some(0));
    state
}

pub(super) fn scroll_table(state: &mut TableState, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let current = state.selected().unwrap_or(0) as isize;
    let next = (current + delta).clamp(0, (len - 1) as isize) as usize;
    state.select(Some(next));
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
