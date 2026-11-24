//! TUI 键盘与鼠标输入事件分发。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::{scroll_table, ActiveModal, App, ChartMode, FocusPane, KeyAction, Tab};

impl App {
    /// 处理单次键盘事件。
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return KeyAction::Quit;
        }

        // 任意弹窗打开时独占键盘输入
        if let Some(modal) = self.active_modal {
            self.handle_modal_key(modal, key);
            return KeyAction::None;
        }

        match key.code {
            KeyCode::Char('a') => {
                self.open_agent_picker();
                KeyAction::None
            }
            KeyCode::Char('u') => {
                self.open_unknown_models();
                KeyAction::None
            }
            KeyCode::Char('t') if self.current_tab == Tab::Overview => {
                self.chart_mode = self.chart_mode.toggle();
                KeyAction::None
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter => {
                self.toggle_focus_pane();
                KeyAction::None
            }
            KeyCode::Esc => {
                if self.current_tab.has_dual_panes() && self.focus == FocusPane::Lower {
                    self.focus = FocusPane::Upper;
                    KeyAction::None
                } else {
                    KeyAction::Quit
                }
            }
            KeyCode::Char('q') => KeyAction::Quit,
            KeyCode::Right | KeyCode::Char('l') => {
                self.switch_tab(self.current_tab.next());
                KeyAction::None
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.switch_tab(self.current_tab.prev());
                KeyAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_focused(-1);
                KeyAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_focused(1);
                KeyAction::None
            }
            KeyCode::PageUp => {
                self.scroll_focused(-10);
                KeyAction::None
            }
            KeyCode::PageDown => {
                self.scroll_focused(10);
                KeyAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.jump_focused(0);
                KeyAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                let len = self.focused_list_len();
                if len > 0 {
                    self.jump_focused(len - 1);
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

    /// 处理单次鼠标事件（左键点击选中 / 切换 Tab / 开关弹窗，滚轮悬停滚动）。
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;

        // 弹窗打开时由弹窗独占鼠标交互
        if let Some(modal) = self.active_modal {
            self.handle_modal_mouse(modal, mouse);
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_left_click(col, row),
            MouseEventKind::ScrollUp => self.handle_mouse_scroll(col, row, -1),
            MouseEventKind::ScrollDown => self.handle_mouse_scroll(col, row, 1),
            _ => {}
        }
    }

    /// 处理主界面左键点击命中。
    fn handle_left_click(&mut self, col: u16, row: u16) {
        if let Some(btn_rect) = self.layout.agent_button_rect {
            if rect_contains(btn_rect, col, row) {
                self.open_agent_picker();
                return;
            }
        }

        if let Some(badge_rect) = self.layout.unknown_badge_rect {
            if rect_contains(badge_rect, col, row) {
                self.open_unknown_models();
                return;
            }
        }

        if let Some(days_rect) = self.layout.chart_days_rect {
            if rect_contains(days_rect, col, row) {
                self.chart_mode = ChartMode::Days;
                return;
            }
        }

        if let Some(hours_rect) = self.layout.chart_hours_rect {
            if rect_contains(hours_rect, col, row) {
                self.chart_mode = ChartMode::Hours;
                return;
            }
        }

        for &(tab, rect) in &self.layout.tab_rects {
            if rect_contains(rect, col, row) {
                self.switch_tab(tab);
                return;
            }
        }

        if let Some(upper) = self.layout.upper {
            if rect_contains(upper.panel, col, row) && self.current_tab.has_dual_panes() {
                self.focus = FocusPane::Upper;
                if let Some(rows_rect) = upper.rows {
                    if rect_contains(rows_rect, col, row) {
                        self.click_upper_row((row - rows_rect.y) as usize);
                    }
                }
                return;
            }
        }

        if let Some(lower) = self.layout.lower {
            if rect_contains(lower.panel, col, row) {
                if self.current_tab.has_dual_panes() && self.lower_list_len() > 0 {
                    self.focus = FocusPane::Lower;
                }
                if let Some(rows_rect) = lower.rows {
                    if rect_contains(rows_rect, col, row) {
                        self.click_lower_row((row - rows_rect.y) as usize);
                    }
                }
            }
        }
    }

    /// 处理弹窗打开时的鼠标点击与滚轮事件。
    fn handle_modal_mouse(&mut self, modal: ActiveModal, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;
        let len = self.modal_list_len();

        match mouse.kind {
            MouseEventKind::ScrollUp => scroll_table(&mut self.modal_table, len, -1),
            MouseEventKind::ScrollDown => scroll_table(&mut self.modal_table, len, 1),
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = match self.layout.modal {
                    Some(m) => m,
                    None => return,
                };
                if let Some(rows_rect) = hit.rows {
                    if rect_contains(rows_rect, col, row) {
                        let idx = self.modal_table.offset() + (row - rows_rect.y) as usize;
                        if idx < len {
                            self.modal_table.select(Some(idx));
                            if modal == ActiveModal::AgentPicker {
                                self.confirm_agent_picker();
                            }
                        }
                        return;
                    }
                }
                if !rect_contains(hit.panel, col, row) {
                    self.active_modal = None;
                }
            }
            _ => {}
        }
    }

    /// 根据鼠标悬停位置滚动对应的子框（未命中任何框时滚动当前焦点框）。
    fn handle_mouse_scroll(&mut self, col: u16, row: u16, delta: isize) {
        if let Some(upper) = self.layout.upper {
            if rect_contains(upper.panel, col, row) && self.current_tab.has_dual_panes() {
                self.scroll_upper(delta);
                return;
            }
        }
        if let Some(lower) = self.layout.lower {
            if rect_contains(lower.panel, col, row) {
                self.scroll_lower(delta);
                return;
            }
        }
        self.scroll_focused(delta);
    }

    /// 处理当前打开弹窗内的键盘操作。
    fn handle_modal_key(&mut self, modal: ActiveModal, key: KeyEvent) {
        let len = self.modal_list_len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.active_modal = None;
            }
            KeyCode::Char('a') if modal == ActiveModal::AgentPicker => {
                self.active_modal = None;
            }
            KeyCode::Char('u') if modal == ActiveModal::UnknownModels => {
                self.active_modal = None;
            }
            KeyCode::Enter | KeyCode::Char(' ') => match modal {
                ActiveModal::AgentPicker => self.confirm_agent_picker(),
                ActiveModal::UnknownModels => self.active_modal = None,
            },
            KeyCode::Up | KeyCode::Char('k') => {
                scroll_table(&mut self.modal_table, len, -1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                scroll_table(&mut self.modal_table, len, 1);
            }
            KeyCode::PageUp => {
                scroll_table(&mut self.modal_table, len, -10);
            }
            KeyCode::PageDown => {
                scroll_table(&mut self.modal_table, len, 10);
            }
            KeyCode::Home | KeyCode::Char('g') if len > 0 => {
                self.modal_table.select(Some(0));
            }
            KeyCode::End | KeyCode::Char('G') if len > 0 => {
                self.modal_table.select(Some(len - 1));
            }
            _ => {}
        }
    }
}

/// 判断终端坐标 `(col, row)` 是否落在矩形 `rect` 内。
fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}
