//! TUI 三段式顶层渲染与公共视觉辅助。
//!
//! 布局结构：
//! - 顶部（3 行）：Tabs 导航栏（`Overview` | `Day`）
//! - 中间（弹性）：当前激活视图内容（首次加载中显示居中动画）
//! - 底部（1 行）：快捷键提示与 60s 自动刷新状态

mod day;
mod models_panel;
mod overview;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Tabs};
use ratatui::Frame;

use super::app::{App, Tab};

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
            Tab::Day => day::render(frame, app, sections[1]),
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

    let block = styled_block("token-counter").title_top(
        Line::from(Span::styled(
            " opencode · dsh · agy ",
            Style::default().fg(Color::DarkGray),
        ))
        .right_aligned(),
    );

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
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let hints = Line::from(vec![
        Span::styled(" Tab/←/→", Style::default().fg(Color::Cyan)),
        Span::styled(" tab  ", Style::default().fg(Color::DarkGray)),
        Span::styled("↑/↓/j/k", Style::default().fg(Color::Cyan)),
        Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
        Span::styled("r", Style::default().fg(Color::Cyan)),
        Span::styled(" refresh  ", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(hints), chunks[0]);

    let status = if app.refreshing {
        Line::from(vec![
            Span::styled(
                spinner_frame(app.spinner_tick),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(" Refreshing... ", Style::default().fg(Color::DarkGray)),
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

    frame.render_widget(Paragraph::new(status).alignment(Alignment::Right), chunks[1]);
}

// ============================================================================
// 公共样式与数值格式化辅助
// ============================================================================

/// 统一将主区域按 60% / 40% 拆分为「上半主视图 + 底部统一 Models 面板」，
/// 确保在 `Overview` 与 `Day` 之间切 Tab 时底部 Models 框高度完全对齐不跳动。
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
/// 确保 `In / Out / Reason / Cache / Tokens` 全部同量纲且小数点垂直对齐。
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

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use token_counter::core::model::Agent;
    use token_counter::report::aggregate::BucketRow;

    use super::*;
    use crate::tui::data::{ChartDay, ChartSegment, ModelRow, TuiSnapshot};

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

        let model_row = ModelRow {
            agent: Agent::Agy,
            provider: "google".to_string(),
            model: "gemini-4-neon".to_string(),
            metrics: day_row.clone(),
        };

        let mut daily_breakdowns = HashMap::new();
        daily_breakdowns.insert("2026-11-24".to_string(), vec![model_row.clone()]);

        TuiSnapshot {
            overall_total: day_row.clone(),
            daily_rows: vec![day_row],
            daily_breakdowns,
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
            updated_at: "09:38:00".to_string(),
            load_ms: 42,
        }
    }

    #[test]
    fn renders_loading_overview_and_day_views_without_panic() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();

        // 首次加载态
        terminal.draw(|f| render(f, &mut app)).unwrap();

        // 填充快照后渲染 Overview 与 Day 两个 Tab
        app.apply_snapshot(sample_snapshot());
        terminal.draw(|f| render(f, &mut app)).unwrap();

        app.current_tab = Tab::Day;
        terminal.draw(|f| render(f, &mut app)).unwrap();

        let rendered = terminal.backend().buffer().clone();
        let content: String = rendered.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("Overview"));
        assert!(content.contains("Day"));
        assert!(content.contains("2026-11-24"));
        assert!(content.contains("agy · google · gemini-4-neon"));
    }
}
