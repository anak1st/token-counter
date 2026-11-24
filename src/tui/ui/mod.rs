//! TUI 三段式顶层渲染与公共视觉辅助。
//!
//! 布局结构：
//! - 顶部（3 行）：Tabs 导航栏（`Overview` | `Hour` | `Day` | `Week` | `Month` | `Session`）
//! - 中间（弹性）：当前激活视图内容（首次加载中显示居中动画）
//! - 底部（1 行）：快捷键提示与两段式 60s 自动刷新状态

mod models_panel;
mod overview;
mod period;
mod session;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Tabs};
use ratatui::Frame;
use token_counter::core::model::Agent;

use super::app::{App, SessionFocus, Tab};

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

/// 主入口：绘制整帧 TUI。
pub fn render(frame: &mut Frame, app: &mut App) {
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
            Tab::Overview => overview::render(frame, app, sections[1]),
            Tab::Hour => {
                let data = app.snapshot.as_ref().unwrap().hourly.clone();
                period::render(
                    frame,
                    sections[1],
                    period::PeriodViewSpec {
                        panel_name: "Hourly",
                        unit_name: "hours",
                        first_col: "Hour",
                    },
                    &data,
                    &mut app.hour_table,
                );
            }
            Tab::Day => {
                let data = app.snapshot.as_ref().unwrap().daily.clone();
                period::render(
                    frame,
                    sections[1],
                    period::PeriodViewSpec {
                        panel_name: "Daily",
                        unit_name: "days",
                        first_col: "Date",
                    },
                    &data,
                    &mut app.day_table,
                );
            }
            Tab::Week => {
                let data = app.snapshot.as_ref().unwrap().weekly.clone();
                period::render(
                    frame,
                    sections[1],
                    period::PeriodViewSpec {
                        panel_name: "Weekly",
                        unit_name: "weeks",
                        first_col: "Week",
                    },
                    &data,
                    &mut app.week_table,
                );
            }
            Tab::Month => {
                let data = app.snapshot.as_ref().unwrap().monthly.clone();
                period::render(
                    frame,
                    sections[1],
                    period::PeriodViewSpec {
                        panel_name: "Monthly",
                        unit_name: "months",
                        first_col: "Month",
                    },
                    &data,
                    &mut app.month_table,
                );
            }
            Tab::Session => session::render(frame, app, sections[1]),
        }
    }

    render_footer(frame, app, sections[2]);
}

/// 顶部 Tabs 导航栏。
fn render_header(frame: &mut Frame, app: &App, area: Rect) {
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

    let block = styled_block("token-counter");

    let tabs = Tabs::new(titles)
        .block(block)
        .select(selected)
        .divider(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));

    frame.render_widget(tabs, area);
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
    let mut hint_spans = vec![
        Span::styled(" Tab/←/→", Style::default().fg(Color::Cyan)),
        Span::styled(" tab  ", Style::default().fg(Color::DarkGray)),
    ];

    if app.current_tab == Tab::Session {
        let agent_key = format!("a/1-{}", Agent::ALL.len());
        hint_spans.push(Span::styled(agent_key, Style::default().fg(Color::Cyan)));
        hint_spans.push(Span::styled(" agent  ", Style::default().fg(Color::DarkGray)));
        hint_spans.push(Span::styled("↑/↓/j/k", Style::default().fg(Color::Cyan)));
        hint_spans.push(Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)));
        if app.session_focus == SessionFocus::Turns {
            hint_spans.push(Span::styled("Enter/Esc", Style::default().fg(Color::Cyan)));
            hint_spans.push(Span::styled(" back  ", Style::default().fg(Color::DarkGray)));
        } else {
            hint_spans.push(Span::styled("Enter", Style::default().fg(Color::Cyan)));
            hint_spans.push(Span::styled(" turns  ", Style::default().fg(Color::DarkGray)));
        }
    } else {
        hint_spans.push(Span::styled("↑/↓/j/k", Style::default().fg(Color::Cyan)));
        hint_spans.push(Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)));
    }

    hint_spans.push(Span::styled("r", Style::default().fg(Color::Cyan)));
    hint_spans.push(Span::styled(" refresh  ", Style::default().fg(Color::DarkGray)));
    hint_spans.push(Span::styled("q", Style::default().fg(Color::Cyan)));
    hint_spans.push(Span::styled(" quit", Style::default().fg(Color::DarkGray)));

    let hints = Line::from(hint_spans);

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
        let mut spans = Vec::new();
        if !snap.unknown_models.is_empty() {
            spans.push(Span::styled(
                format!("{} unknown · ", snap.unknown_models.len()),
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

/// 统一的表格选中行高亮样式。
pub fn highlight_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
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

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use token_counter::core::model::Agent;
    use token_counter::report::aggregate::BucketRow;
    use token_counter::report::session::{SessionDetail, SessionSummary, TurnSummary};

    use super::*;
    use crate::tui::app::KeyAction;
    use crate::tui::data::{ChartDay, ChartSegment, ModelRow, PeriodData, TuiSnapshot};

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
            agent: Agent::Agy,
            provider: "google".to_string(),
            model: "gemini-4-neon".to_string(),
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

        let mut sessions_by_agent: HashMap<Agent, Vec<SessionDetail>> = HashMap::new();
        sessions_by_agent.insert(
            Agent::Dsh,
            vec![SessionDetail {
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
            }],
        );

        TuiSnapshot {
            overall_total: day_row.clone(),
            hourly: make_period(hour_row),
            daily: make_period(day_row),
            weekly: make_period(week_row),
            monthly: make_period(month_row),
            sessions_by_agent,
            model_rows: vec![model_row],
            chart_days: vec![ChartDay {
                short_label: "11-24".to_string(),
                total_tokens: 4_600_000,
                segments: vec![ChartSegment {
                    model: "gemini-4-neon".to_string(),
                    tokens: 4_600_000,
                }],
            }],
            unknown_models: Vec::new(),
            is_partial: false,
            updated_at: "09:38:00".to_string(),
            load_ms: 42,
        }
    }

    #[test]
    fn renders_loading_and_all_tabs_without_panic() {
        let backend = TestBackend::new(136, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();

        // 首次加载态
        terminal.draw(|f| render(f, &mut app)).unwrap();

        // 填充快照后依次渲染全部 6 个 Tab
        app.apply_snapshot(sample_snapshot());
        for tab in Tab::ALL {
            app.current_tab = tab;
            terminal.draw(|f| render(f, &mut app)).unwrap();
        }

        // 在 `Session` Tab 下通过快捷键切换 Agent 并进入 Turn 焦点
        app.current_tab = Tab::Session;
        assert_eq!(app.session_agent, Agent::Opencode);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)),
            KeyAction::None
        );
        assert_eq!(app.session_agent, Agent::Dsh);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            KeyAction::None
        );
        assert_eq!(app.session_focus, SessionFocus::Turns);

        terminal.draw(|f| render(f, &mut app)).unwrap();
        let rendered = terminal.backend().buffer().clone();
        let content: String = rendered.content().iter().map(|c| c.symbol()).collect();

        assert!(content.contains("Overview"));
        assert!(content.contains("Month"));
        assert!(content.contains("Session"));
        assert!(content.contains("2:dsh (1)"));
        assert!(content.contains("019ab3c4 · token-counter"));
        assert!(content.contains("Turns · 019ab3c4-dem · token-counter (1 turns)"));
        assert!(content.contains("#1"));
        assert!(!content.contains("opencode · dsh · agy"));

        // Esc 先从 Turns 退回 Sessions，不直接退出
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            KeyAction::None
        );
        assert_eq!(app.session_focus, SessionFocus::Sessions);
    }
}
