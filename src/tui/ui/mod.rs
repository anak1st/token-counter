//! TUI 三段式顶层渲染与公共视觉辅助。
//!
//! 布局结构：
//! - 顶部（3 行）：Tabs 导航栏（`Overview` | `Hour` | `Day` | `Week` | `Month` | `Session`）
//! - 中间（弹性）：当前激活视图内容（首次加载中显示居中动画）
//! - 底部（1 行）：快捷键提示与两段式 60s 自动刷新状态

mod agent_picker;
mod models_panel;
mod overview;
mod period;
mod session;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Tabs,
};
use ratatui::Frame;

use super::app::{App, FocusPane, Tab, UiLayout};

/// 加载动画帧序列。
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 模型配色表（兼容绝大多数深浅终端的 ANSI 命名色）。
const MODEL_PALETTE: [Color; 7] = [
    Color::Cyan,
    Color::Magenta,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::LightRed,
    Color::LightCyan,
];

/// 主入口：绘制整帧 TUI 并刷新本帧鼠标交互热区。
pub fn render(frame: &mut Frame, app: &mut App) {
    app.layout = UiLayout::default();

    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, sections[0]);

    if app.snapshot.is_none() {
        render_loading(frame, app, sections[1]);
    } else {
        match app.current_tab {
            Tab::Overview => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let lower_hit = overview::render(
                        frame,
                        sections[1],
                        view,
                        snap.is_partial,
                        &mut app.overview_table,
                    );
                    app.layout.lower = Some(lower_hit);
                }
            }
            Tab::Hour => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Hourly",
                            unit_name: "hours",
                            first_col: "Hour",
                        },
                        &view.hourly,
                        &mut app.hour_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Day => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Daily",
                            unit_name: "days",
                            first_col: "Date",
                        },
                        &view.daily,
                        &mut app.day_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Week => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Weekly",
                            unit_name: "weeks",
                            first_col: "Week",
                        },
                        &view.weekly,
                        &mut app.week_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Month => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Monthly",
                            unit_name: "months",
                            first_col: "Month",
                        },
                        &view.monthly,
                        &mut app.month_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Session => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = session::render(
                        frame,
                        sections[1],
                        &view.sessions,
                        app.agent_filter.is_none(),
                        &mut app.session_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = Some(upper_hit);
                    app.layout.lower = Some(lower_hit);
                }
            }
        }
    }

    render_footer(frame, app, sections[2]);

    // 弹窗打开时覆盖渲染在最顶层
    if app.agent_picker_open {
        app.layout.modal = agent_picker::render(frame, app, area);
    }
}

/// 顶部 Tabs 导航栏：边框与其余面板完全一致，右侧框内放置紧凑的 Agent 选择按钮。
fn render_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = styled_block("token-counter");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|&tab| {
            let style = if tab == app.current_tab {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(tab.title(), style))
        })
        .collect();

    let selected = Tab::ALL
        .iter()
        .position(|&t| t == app.current_tab)
        .unwrap_or(0);

    let agent_value_style = if app.agent_filter.is_some() {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let agent_button = Line::from(vec![
        Span::styled("Agent: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.active_agent_label(), agent_value_style),
        Span::styled(" [a] ", Style::default().fg(Color::Cyan)),
    ]);

    let button_width = (agent_button.width() as u16).min(inner.width / 2);
    let inner_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(button_width)])
        .split(inner);

    // 精确记录每个 Tab 与右侧 Agent 按钮的鼠标点击热区
    let mut cursor_x = inner_chunks[0].x;
    let tabs_right = inner_chunks[0].right();
    for (idx, &tab) in Tab::ALL.iter().enumerate() {
        if idx > 0 {
            cursor_x = cursor_x.saturating_add(1);
        }
        let tab_width = (tab.title().len() as u16).saturating_add(2);
        if cursor_x < tabs_right {
            let clamped_w = tab_width.min(tabs_right - cursor_x);
            app.layout
                .tab_rects
                .push((tab, Rect::new(cursor_x, inner_chunks[0].y, clamped_w, 1)));
        }
        cursor_x = cursor_x.saturating_add(tab_width);
    }
    app.layout.agent_button_rect = Some(inner_chunks[1]);

    let tabs = Tabs::new(titles)
        .select(selected)
        .padding(" ", " ")
        .divider(Span::styled("│", Style::default().fg(Color::DarkGray)));

    frame.render_widget(tabs, inner_chunks[0]);
    frame.render_widget(
        Paragraph::new(agent_button).alignment(Alignment::Right),
        inner_chunks[1],
    );
}

/// 首次后台扫描完成前的居中加载态。
fn render_loading(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Loading");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Length(1),
            Constraint::Percentage(45),
        ])
        .split(inner);

    let line = if let Some(ref err) = app.last_error {
        Line::from(Span::styled(
            format!("Error: {}", err),
            Style::default().fg(Color::Red),
        ))
    } else {
        Line::from(vec![
            Span::styled(
                spinner_frame(app.spinner_tick),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "Scanning local agent sessions...",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };

    let paragraph = Paragraph::new(line).alignment(Alignment::Center);
    frame.render_widget(paragraph, rows[1]);
}

/// 底部提示与 60s 刷新状态栏。
fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = if app.agent_picker_open {
        Line::from(vec![
            Span::styled(" ↑/↓/j/k", Style::default().fg(Color::Cyan)),
            Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" confirm  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc/a", Style::default().fg(Color::Cyan)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        let mut hint_spans = vec![
            Span::styled(" ←/→", Style::default().fg(Color::Cyan)),
            Span::styled(" view  ", Style::default().fg(Color::DarkGray)),
        ];

        if app.current_tab.has_dual_panes() {
            hint_spans.push(Span::styled("Tab", Style::default().fg(Color::Cyan)));
            if app.focus == FocusPane::Lower {
                hint_spans.push(Span::styled(" upper  ", Style::default().fg(Color::DarkGray)));
            } else {
                hint_spans.push(Span::styled(" pane  ", Style::default().fg(Color::DarkGray)));
            }
        }

        hint_spans.push(Span::styled("a", Style::default().fg(Color::Cyan)));
        hint_spans.push(Span::styled(" agent  ", Style::default().fg(Color::DarkGray)));
        hint_spans.push(Span::styled("↑/↓", Style::default().fg(Color::Cyan)));
        hint_spans.push(Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)));
        hint_spans.push(Span::styled("r", Style::default().fg(Color::Cyan)));
        hint_spans.push(Span::styled(" refresh  ", Style::default().fg(Color::DarkGray)));
        hint_spans.push(Span::styled("q", Style::default().fg(Color::Cyan)));
        hint_spans.push(Span::styled(" quit", Style::default().fg(Color::DarkGray)));
        Line::from(hint_spans)
    };

    let status = if app.refreshing {
        let is_backfilling = app.snapshot.as_ref().is_some_and(|s| s.is_partial);
        let msg = if is_backfilling {
            " 30d ready · loading history... "
        } else {
            " Refreshing 30d... "
        };
        Line::from(vec![
            Span::styled(
                spinner_frame(app.spinner_tick),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(msg, Style::default().fg(Color::DarkGray)),
        ])
    } else if let Some(ref err) = app.last_error {
        Line::from(Span::styled(
            format!("Error: {} ", err),
            Style::default().fg(Color::Red),
        ))
    } else if let Some(ref snap) = app.snapshot {
        let unknown_count = snap.view(app.agent_filter).unknown_models.len();

        let mut spans = Vec::new();
        if unknown_count > 0 {
            spans.push(Span::styled(
                format!("{} unknown · ", unknown_count),
                Style::default().fg(Color::Yellow),
            ));
        }
        spans.push(Span::styled(
            format!(
                "Updated {} ({}ms) · next {}s ",
                snap.updated_at,
                snap.load_ms,
                app.secs_until_next_refresh()
            ),
            Style::default().fg(Color::DarkGray),
        ));
        Line::from(spans)
    } else {
        Line::default()
    };

    let status_width = (status.width() as u16).min(area.width / 2);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(status_width)])
        .split(area);

    frame.render_widget(Paragraph::new(hints), chunks[0]);
    frame.render_widget(Paragraph::new(status).alignment(Alignment::Right), chunks[1]);
}

// ============================================================================
// 公共样式与数值格式化辅助
// ============================================================================

/// 统一将主区域按 60% / 40% 拆分为「上半主视图 + 下半联动明细面板」，
/// 确保在各个 Tab 之间切换时底部框高度完全对齐不跳动。
pub fn split_upper_and_models(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    (chunks[0], chunks[1])
}

/// 创建统一圆角边框面板（透明底色，自然融入用户终端主题）。
pub fn styled_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
}

/// 创建带聚焦状态切换的圆角边框面板（聚焦时纯白亮框，失焦时深灰边框）。
pub fn panel_block(title: &str, focused: bool) -> Block<'_> {
    if focused {
        styled_block(title).border_style(Style::default().fg(Color::White))
    } else {
        styled_block(title)
    }
}

/// 从带 1 行表头的表格内区算出实际数据行区域（供鼠标行点击命中计算）。
pub fn table_rows_rect(inner: Rect) -> Rect {
    Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    }
}

/// 统一的表格选中行高亮样式（当前聚焦框）。
pub fn highlight_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

/// 上半主表行高亮样式：聚焦时青色高亮，失焦时暗灰反白以保留当前选中分桶/会话指示。
pub fn upper_highlight_style(focused: bool) -> Style {
    if focused {
        highlight_style()
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::REVERSED)
    }
}

/// 当表格数据行数超过可视区域时，在右边框绘制垂直滚动条。
pub fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    total_rows: usize,
    selected: Option<usize>,
) {
    let visible_rows = inner.height.saturating_sub(1) as usize;
    if total_rows > visible_rows {
        let mut sb_state = ScrollbarState::new(total_rows).position(selected.unwrap_or(0));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb_state,
        );
    }
}

/// 统一的表格表头样式。
pub fn table_header_style() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// 为模型分配确定性颜色（同一模型在柱状图、Overview 与 Day 明细中保持同色）。
pub fn model_color(model: &str) -> Color {
    if model.contains("deepseek") {
        return Color::Cyan;
    }
    if model.contains("neon") {
        return Color::Magenta;
    }
    if model.contains("hydrogen") {
        return Color::Green;
    }
    if model.contains("glm") {
        return Color::Yellow;
    }
    if model.contains("argon") || model.contains("carbon") {
        return Color::Blue;
    }
    let mut hash: usize = 0;
    for b in model.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(usize::from(b));
    }
    MODEL_PALETTE[hash % MODEL_PALETTE.len()]
}

pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

/// 千分位逗号整数格式化（如 `138,557,002`）。
pub fn format_int(n: u64) -> String {
    let raw = n.to_string();
    let len = raw.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, ch) in raw.chars().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 统一按百万单位 `M` 格式化 token（保留 3 位小数，精确到 1k），
/// 确保 `In / Out / Cache / Tokens` 全部同量纲且小数点垂直对齐。
pub fn format_tokens_m(n: u64) -> String {
    if n == 0 {
        return "—".to_string();
    }
    if n < 500 {
        return "<0.001M".to_string();
    }
    format!("{:.3}M", n as f64 / 1_000_000.0)
}

/// 紧凑 token 格式化（仅用于柱状图 Y 轴刻度，如 `195M`、`42k`）。
pub fn format_axis_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.0}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_cost(usd: f64) -> String {
    format!("${:.3}", usd)
}

pub fn format_cache(pct: Option<f64>) -> String {
    match pct {
        Some(v) => format!("{:.1}%", v),
        None => "—".to_string(),
    }
}

pub fn format_ttft(secs: Option<f64>) -> String {
    match secs {
        Some(v) => format!("{:.1}s", v),
        None => "—".to_string(),
    }
}

pub fn format_speed(tps: Option<f64>) -> String {
    match tps {
        Some(v) => format!("{}/s", v.round() as u64),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use token_counter::core::model::Agent;
    use token_counter::report::aggregate::BucketRow;
    use token_counter::report::session::{SessionDetail, SessionSummary, TurnSummary};

    use super::*;
    use crate::tui::app::KeyAction;
    use crate::tui::data::{
        ChartDay, ChartSegment, ModelRow, PeriodData, SessionItem, TuiSnapshot, ViewSnapshot,
    };

    fn empty_view() -> ViewSnapshot {
        ViewSnapshot {
            overall_total: BucketRow::empty("Total"),
            hourly: PeriodData {
                rows: Vec::new(),
                breakdowns: HashMap::new(),
            },
            daily: PeriodData {
                rows: Vec::new(),
                breakdowns: HashMap::new(),
            },
            weekly: PeriodData {
                rows: Vec::new(),
                breakdowns: HashMap::new(),
            },
            monthly: PeriodData {
                rows: Vec::new(),
                breakdowns: HashMap::new(),
            },
            model_rows: Vec::new(),
            chart_days: Vec::new(),
            sessions: Vec::new(),
            unknown_models: Vec::new(),
        }
    }

    fn sample_snapshot() -> TuiSnapshot {
        let mut day_row = BucketRow::empty("2026-11-24");
        day_row.requests = 12;
        day_row.input = 500_000;
        day_row.output = 100_000;
        day_row.reasoning = 40_000;
        day_row.cache_read = 4_000_000;
        day_row.cost_usd = 2.5;
        day_row.ttft_sum_ms = 5_000;
        day_row.ttft_count = 2;
        day_row.gen_ms = 1_000;
        day_row.speed_tokens = 120;

        let mut hour_row = day_row.clone();
        hour_row.label = "2026-11-24 10:00".to_string();

        let mut week_row = day_row.clone();
        week_row.label = "2026-W48".to_string();

        let mut month_row = day_row.clone();
        month_row.label = "2026-11".to_string();

        let model_row = ModelRow {
            agent: Agent::Dsh,
            provider: "deepseek-official".to_string(),
            model: "deepseek-v4.1-flash".to_string(),
            metrics: day_row.clone(),
        };

        let make_period = |row: BucketRow| {
            let mut breakdowns = HashMap::new();
            breakdowns.insert(row.label.clone(), vec![model_row.clone()]);
            PeriodData {
                rows: vec![row],
                breakdowns,
            }
        };

        let dsh_session = SessionItem {
            agent: Agent::Dsh,
            detail: SessionDetail {
                summary: SessionSummary {
                    session_id: "019ab3c4-demo-session".to_string(),
                    label: Some("token-counter".to_string()),
                    first_ts_ms: 1_763_949_600_000,
                    last_ts_ms: 1_763_950_200_000,
                    models: vec!["deepseek-v4.1-flash".to_string()],
                    turns: 1,
                    steps: 3,
                    aborted_steps: 0,
                    peak_ctx: 162_000,
                    input: 100_000,
                    output: 20_000,
                    reasoning: 0,
                    cache_read: 500_000,
                    cache_write: 0,
                    cost_usd: 0.42,
                    ttft_sum_ms: 3_000,
                    ttft_count: 2,
                    gen_ms: 1_000,
                    speed_tokens: 110,
                },
                turns: vec![TurnSummary {
                    index: 1,
                    turn_id: "1".to_string(),
                    first_ts_ms: 1_763_949_600_000,
                    last_ts_ms: 1_763_950_200_000,
                    models: vec!["deepseek-v4.1-flash".to_string()],
                    steps: 3,
                    aborted_steps: 0,
                    peak_ctx: 162_000,
                    input: 100_000,
                    output: 20_000,
                    reasoning: 0,
                    cache_read: 500_000,
                    cache_write: 0,
                    cost_usd: 0.42,
                    ttft_sum_ms: 3_000,
                    ttft_count: 2,
                    gen_ms: 1_000,
                    speed_tokens: 110,
                }],
            },
        };

        let dsh_view = ViewSnapshot {
            overall_total: day_row.clone(),
            hourly: make_period(hour_row),
            daily: make_period(day_row),
            weekly: make_period(week_row),
            monthly: make_period(month_row),
            model_rows: vec![model_row],
            chart_days: vec![ChartDay {
                short_label: "11-24".to_string(),
                total_tokens: 4_600_000,
                segments: vec![ChartSegment {
                    model: "deepseek-v4.1-flash".to_string(),
                    tokens: 4_600_000,
                }],
            }],
            sessions: vec![dsh_session],
            unknown_models: Vec::new(),
        };

        let mut by_agent: HashMap<Agent, ViewSnapshot> = HashMap::new();
        by_agent.insert(Agent::Opencode, empty_view());
        by_agent.insert(Agent::Dsh, dsh_view.clone());
        by_agent.insert(Agent::Agy, empty_view());

        TuiSnapshot {
            all: dsh_view,
            by_agent,
            is_partial: false,
            updated_at: "09:38:00".to_string(),
            load_ms: 42,
        }
    }

    #[test]
    fn renders_loading_all_tabs_and_agent_picker_modal() {
        let backend = TestBackend::new(136, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();

        // 首次加载态
        terminal.draw(|f| render(f, &mut app)).unwrap();

        // 填充快照后依次渲染全部 6 个 Tab（默认 All 模式，含 Session）
        app.apply_snapshot(sample_snapshot());
        for tab in Tab::ALL {
            app.current_tab = tab;
            terminal.draw(|f| render(f, &mut app)).unwrap();
        }

        // 在 `Day` Tab 下通过 `Tab` 切到下方 Models 子框，再通过 Esc 退回上表
        app.switch_tab(Tab::Day);
        assert_eq!(app.focus, FocusPane::Upper);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, FocusPane::Lower);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            KeyAction::None
        );
        assert_eq!(app.focus, FocusPane::Upper);

        // 鼠标点击顶部 `Session` Tab 切换视图
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let session_tab_rect = app
            .layout
            .tab_rects
            .iter()
            .find(|(t, _)| *t == Tab::Session)
            .unwrap()
            .1;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: session_tab_rect.x + 1,
            row: session_tab_rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.current_tab, Tab::Session);

        // `Session` Tab 默认处于 All 模式：标题为 `Sessions (1)`，行首带 agent 前缀
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let all_session_buf: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(all_session_buf.contains("Sessions (1)"));
        assert!(all_session_buf.contains("Agent: all [a]"));
        assert!(all_session_buf.contains("dsh · 019ab3c4"));
        assert!(all_session_buf.contains("Turns · dsh · 019ab3c4-dem · token-counter (1 turns)"));

        // 鼠标点击顶部 `Agent: all [a]` 按钮打开弹窗，并点击弹窗中的 `dsh` 行确认
        let btn_rect = app.layout.agent_button_rect.unwrap();
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: btn_rect.x + 1,
            row: btn_rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.agent_picker_open);
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let modal_buf: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(modal_buf.contains("Select Agent"));
        assert!(modal_buf.contains("all · All Agents"));
        assert!(modal_buf.contains("dsh · DeepSeek Harness"));

        let modal_rows = app.layout.modal.unwrap().rows.unwrap();
        // 第 2 行索引（all=0, opencode=1, dsh=2）
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: modal_rows.x + 2,
            row: modal_rows.y + 2,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!app.agent_picker_open);
        assert_eq!(app.agent_filter, Some(Agent::Dsh));

        // 鼠标点击下方 Turns 面板：焦点自动切到 Lower
        terminal.draw(|f| render(f, &mut app)).unwrap();
        let lower_rows = app.layout.lower.unwrap().rows.unwrap();
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: lower_rows.x + 2,
            row: lower_rows.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.focus, FocusPane::Lower);

        terminal.draw(|f| render(f, &mut app)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("Agent: dsh [a]"));
        assert!(content.contains("Sessions (1)"));
        assert!(content.contains("019ab3c4 · token-counter"));
        assert!(content.contains("Turns · 019ab3c4-dem · token-counter (1 turns)"));
        assert!(content.contains("#1"));

        // Esc 先从下表退回上表，不直接退出
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            KeyAction::None
        );
        assert_eq!(app.focus, FocusPane::Upper);
    }
}
