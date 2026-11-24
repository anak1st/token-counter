//! `Session` Tab 视图：顶部 Agent 切换按钮栏 + 上半区会话列表 + 下半区逐个 Turn 明细。
//!
//! 布局与 `Hour` / `Day` / `Week` / `Month` 严格保持一致（60% / 40% 纵向划分）：
//! - 上半区顶部 1 行为可扩展的 Agent 按钮切换栏（`a` 循环或 `1-N` 直达）；
//! - 上半区主体为当前 Agent 的会话列表（按最后活跃时间倒序）；
//! - 下半区为当前选中会话的逐个 Turn 明细（按时间升序，`Enter` 可切换焦点滚动）。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState,
};
use ratatui::Frame;
use token_counter::core::model::Agent;
use token_counter::core::time::format_datetime;
use token_counter::report::session::{SessionDetail, TurnSummary};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::models_panel::TABLE_COL_SPACING;
use super::{
    format_cache, format_cost, format_speed, format_tokens_m, format_ttft, highlight_style,
    model_color, split_upper_and_models, styled_block, table_header_style,
};
use crate::tui::app::{App, SessionFocus};

/// 展开完整列（含 `PeakCtx / Cache% / TTFT` 与完整年份时间）所需的最小终端宽度。
const WIDE_SESSION_THRESHOLD: u16 = 126;

/// 渲染 `Session` Tab。
pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let (upper_area, lower_area) = split_upper_and_models(area);

    let upper_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(upper_area);

    render_agent_selector(frame, app, upper_chunks[0]);

    let agent = app.session_agent;
    let focus = app.session_focus;
    let sessions: Vec<SessionDetail> = app.sessions_for_agent(agent).to_vec();

    let selected_idx = {
        let state = app.session_tables.entry(agent).or_default();
        render_sessions_table(
            frame,
            upper_chunks[1],
            agent,
            &sessions,
            state,
            focus == SessionFocus::Sessions,
        );
        state.selected()
    };

    let selected_detail = selected_idx.and_then(|idx| sessions.get(idx));
    render_turns_table(
        frame,
        lower_area,
        selected_detail,
        &mut app.turn_table,
        focus == SessionFocus::Turns,
    );
}

/// 顶部单行 Agent 切换按钮栏（随 `Agent::ALL` 自动扩展）。
fn render_agent_selector(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(
        " Agent  ",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ));

    for (idx, &agent) in Agent::ALL.iter().enumerate() {
        let count = app.sessions_for_agent(agent).len();
        let label = format!(" {}:{} ({}) ", idx + 1, agent.id(), count);
        let style = if agent == app.session_agent {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }

    let hint = format!(" a / 1-{} switch", Agent::ALL.len());
    spans.push(Span::styled(hint, Style::default().fg(Color::DarkGray)));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 上半区：当前选中 Agent 的会话列表（按最后活跃时间倒序）。
fn render_sessions_table(
    frame: &mut Frame,
    area: Rect,
    agent: Agent,
    sessions: &[SessionDetail],
    table_state: &mut TableState,
    focused: bool,
) {
    let title = format!("Sessions · {} ({})", agent.id(), sessions.len());
    let mut block = styled_block(&title);
    if focused {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }

    if sessions.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(format!("No sessions found for {}.", agent.id()))
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let inner = block.inner(area);
    let wide = area.width >= WIDE_SESSION_THRESHOLD;
    let header = sessions_header(wide);
    let widths = sessions_widths(wide);
    let model_budget = if wide { 16 } else { 14 };

    let mut rows: Vec<Row> = Vec::with_capacity(sessions.len());
    for detail in sessions {
        let s = &detail.summary;
        let session_text = match s.label.as_deref() {
            Some(label) => format!("{} · {}", short_id(&s.session_id, 8), label),
            None => short_id(&s.session_id, 12),
        };
        let time_text = format_table_time(s.last_ts_ms, wide);
        let model_cell = format_model_cell(&s.models, model_budget);
        let cost_cell = Cell::from(Line::from(format_cost(s.cost_usd)).right_aligned())
            .style(Style::default().fg(Color::Green));
        let speed_cell = Cell::from(Line::from(format_speed(s.avg_speed_tps())).right_aligned())
            .style(Style::default().fg(Color::LightCyan));

        if wide {
            rows.push(Row::new([
                Cell::from(session_text),
                Cell::from(time_text).style(Style::default().fg(Color::DarkGray)),
                model_cell,
                Cell::from(Line::from(s.turns.to_string()).right_aligned()),
                Cell::from(Line::from(s.steps.to_string()).right_aligned()),
                Cell::from(Line::from(format_compact_tokens(s.peak_ctx)).right_aligned()),
                Cell::from(Line::from(format_tokens_m(s.total_tokens())).right_aligned()),
                Cell::from(Line::from(format_cache(s.cache_percent())).right_aligned()),
                cost_cell,
                Cell::from(Line::from(format_ttft(s.avg_ttft_secs())).right_aligned()),
                speed_cell,
            ]));
        } else {
            rows.push(Row::new([
                Cell::from(session_text),
                Cell::from(time_text).style(Style::default().fg(Color::DarkGray)),
                model_cell,
                Cell::from(Line::from(s.turns.to_string()).right_aligned()),
                Cell::from(Line::from(s.steps.to_string()).right_aligned()),
                Cell::from(Line::from(format_tokens_m(s.total_tokens())).right_aligned()),
                cost_cell,
                speed_cell,
            ]));
        }
    }

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(TABLE_COL_SPACING)
        .row_highlight_style(highlight_style());

    frame.render_stateful_widget(table, area, table_state);

    if sessions.len() > inner.height.saturating_sub(1) as usize {
        let selected = table_state.selected().unwrap_or(0);
        let mut sb_state = ScrollbarState::new(sessions.len()).position(selected);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb_state,
        );
    }
}

/// 下半区：当前选中会话的逐个 Turn 轮次明细（按时间升序）。
fn render_turns_table(
    frame: &mut Frame,
    area: Rect,
    detail: Option<&SessionDetail>,
    table_state: &mut TableState,
    focused: bool,
) {
    let detail = match detail {
        Some(d) => d,
        None => {
            let block = styled_block("Turns");
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(
                Paragraph::new("Select a session to inspect turns.")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }
    };

    let s = &detail.summary;
    let title = match s.label.as_deref() {
        Some(label) => format!(
            "Turns · {} · {} ({} turns)",
            short_id(&s.session_id, 12),
            truncate_width(label, 36),
            detail.turns.len()
        ),
        None => format!(
            "Turns · {} ({} turns)",
            short_id(&s.session_id, 12),
            detail.turns.len()
        ),
    };

    let mut block = styled_block(&title);
    if focused {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }

    if detail.turns.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("No turns recorded.").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let inner = block.inner(area);
    let wide = area.width >= WIDE_SESSION_THRESHOLD;
    let header = turns_header(wide);
    let widths = turns_widths(wide);

    let mut rows: Vec<Row> = Vec::with_capacity(detail.turns.len());
    for turn in &detail.turns {
        rows.push(build_turn_row(turn, wide));
    }

    let mut table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(TABLE_COL_SPACING);

    if focused {
        table = table.row_highlight_style(highlight_style());
    }

    frame.render_stateful_widget(table, area, table_state);

    if detail.turns.len() > inner.height.saturating_sub(1) as usize {
        let selected = table_state.selected().unwrap_or(0);
        let mut sb_state = ScrollbarState::new(detail.turns.len()).position(selected);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb_state,
        );
    }
}

/// 构造单条 Turn 明细行。
fn build_turn_row(turn: &TurnSummary, wide: bool) -> Row<'static> {
    let turn_cell = if turn.aborted_steps > 0 {
        Cell::from(format!("#{}*", turn.index)).style(Style::default().fg(Color::Yellow))
    } else {
        Cell::from(format!("#{}", turn.index))
    };

    let time_text = format_table_time(turn.first_ts_ms, wide);
    let model_cell = format_model_cell(&turn.models, 32);
    let span_text = format_span(turn.span_secs());
    let cost_cell = Cell::from(Line::from(format_cost(turn.cost_usd)).right_aligned())
        .style(Style::default().fg(Color::Green));
    let speed_cell = Cell::from(Line::from(format_speed(turn.avg_speed_tps())).right_aligned())
        .style(Style::default().fg(Color::LightCyan));

    if wide {
        Row::new([
            turn_cell,
            Cell::from(time_text).style(Style::default().fg(Color::DarkGray)),
            model_cell,
            Cell::from(Line::from(span_text).right_aligned()),
            Cell::from(Line::from(turn.steps.to_string()).right_aligned()),
            Cell::from(Line::from(format_compact_tokens(turn.peak_ctx)).right_aligned()),
            Cell::from(Line::from(format_tokens_m(turn.total_tokens())).right_aligned()),
            Cell::from(Line::from(format_cache(turn.cache_percent())).right_aligned()),
            cost_cell,
            Cell::from(Line::from(format_ttft(turn.avg_ttft_secs())).right_aligned()),
            speed_cell,
        ])
    } else {
        Row::new([
            turn_cell,
            Cell::from(time_text).style(Style::default().fg(Color::DarkGray)),
            model_cell,
            Cell::from(Line::from(span_text).right_aligned()),
            Cell::from(Line::from(turn.steps.to_string()).right_aligned()),
            Cell::from(Line::from(format_tokens_m(turn.total_tokens())).right_aligned()),
            cost_cell,
            speed_cell,
        ])
    }
}

/// 上半区 `Sessions` 表格列宽（与下半区 `Turns` 的右侧数值列严格垂直对齐）。
fn sessions_widths(wide: bool) -> Vec<Constraint> {
    if wide {
        vec![
            Constraint::Min(18),   // Session (id · label)
            Constraint::Length(16), // Time
            Constraint::Length(18), // Model
            Constraint::Length(6),  // Turns
            Constraint::Length(5),  // Steps
            Constraint::Length(7),  // PeakCtx
            Constraint::Length(9),  // Tokens (M)
            Constraint::Length(7),  // Cache%
            Constraint::Length(9),  // Cost
            Constraint::Length(6),  // TTFT
            Constraint::Length(6),  // Speed
        ]
    } else {
        vec![
            Constraint::Min(14),   // Session
            Constraint::Length(11), // Time (MM-DD HH:MM)
            Constraint::Length(16), // Model
            Constraint::Length(5),  // Turns
            Constraint::Length(5),  // Steps
            Constraint::Length(9),  // Tokens (M)
            Constraint::Length(9),  // Cost
            Constraint::Length(6),  // Speed
        ]
    }
}

/// 下半区 `Turns` 表格列宽（右侧数值列与上表逐列对齐）。
fn turns_widths(wide: bool) -> Vec<Constraint> {
    if wide {
        vec![
            Constraint::Length(6),  // Turn (#1)
            Constraint::Length(16), // Time
            Constraint::Min(18),    // Model
            Constraint::Length(6),  // Span
            Constraint::Length(5),  // Steps
            Constraint::Length(7),  // PeakCtx
            Constraint::Length(9),  // Tokens (M)
            Constraint::Length(7),  // Cache%
            Constraint::Length(9),  // Cost
            Constraint::Length(6),  // TTFT
            Constraint::Length(6),  // Speed
        ]
    } else {
        vec![
            Constraint::Length(5),  // Turn
            Constraint::Length(11), // Time (MM-DD HH:MM)
            Constraint::Min(14),    // Model
            Constraint::Length(5),  // Span
            Constraint::Length(5),  // Steps
            Constraint::Length(9),  // Tokens (M)
            Constraint::Length(9),  // Cost
            Constraint::Length(6),  // Speed
        ]
    }
}

fn sessions_header(wide: bool) -> Row<'static> {
    if wide {
        Row::new([
            Cell::from("Session"),
            Cell::from("Time"),
            Cell::from("Model"),
            Cell::from(Line::from("Turns").right_aligned()),
            Cell::from(Line::from("Steps").right_aligned()),
            Cell::from(Line::from("PeakCtx").right_aligned()),
            Cell::from(Line::from("Tokens").right_aligned()),
            Cell::from(Line::from("Cache%").right_aligned()),
            Cell::from(Line::from("Cost").right_aligned()),
            Cell::from(Line::from("TTFT").right_aligned()),
            Cell::from(Line::from("Speed").right_aligned()),
        ])
        .style(table_header_style())
    } else {
        Row::new([
            Cell::from("Session"),
            Cell::from("Time"),
            Cell::from("Model"),
            Cell::from(Line::from("Turns").right_aligned()),
            Cell::from(Line::from("Steps").right_aligned()),
            Cell::from(Line::from("Tokens").right_aligned()),
            Cell::from(Line::from("Cost").right_aligned()),
            Cell::from(Line::from("Speed").right_aligned()),
        ])
        .style(table_header_style())
    }
}

fn turns_header(wide: bool) -> Row<'static> {
    if wide {
        Row::new([
            Cell::from("Turn"),
            Cell::from("Time"),
            Cell::from("Model"),
            Cell::from(Line::from("Span").right_aligned()),
            Cell::from(Line::from("Steps").right_aligned()),
            Cell::from(Line::from("PeakCtx").right_aligned()),
            Cell::from(Line::from("Tokens").right_aligned()),
            Cell::from(Line::from("Cache%").right_aligned()),
            Cell::from(Line::from("Cost").right_aligned()),
            Cell::from(Line::from("TTFT").right_aligned()),
            Cell::from(Line::from("Speed").right_aligned()),
        ])
        .style(table_header_style())
    } else {
        Row::new([
            Cell::from("Turn"),
            Cell::from("Time"),
            Cell::from("Model"),
            Cell::from(Line::from("Span").right_aligned()),
            Cell::from(Line::from("Steps").right_aligned()),
            Cell::from(Line::from("Tokens").right_aligned()),
            Cell::from(Line::from("Cost").right_aligned()),
            Cell::from(Line::from("Speed").right_aligned()),
        ])
        .style(table_header_style())
    }
}

/// 构造带彩色圆点的模型单元格（多模型时追加 `+N`）。
fn format_model_cell(models: &[String], max_name_width: usize) -> Cell<'static> {
    let primary = match models.first() {
        Some(m) => m.as_str(),
        None => return Cell::from("unknown").style(Style::default().fg(Color::DarkGray)),
    };

    let color = model_color(primary);
    let text = if models.len() > 1 {
        let suffix = format!(" +{}", models.len() - 1);
        let budget = max_name_width.saturating_sub(suffix.len());
        format!("{}{}", truncate_width(primary, budget), suffix)
    } else {
        truncate_width(primary, max_name_width)
    };

    Cell::from(Line::from(vec![
        Span::styled("● ", Style::default().fg(color)),
        Span::raw(text),
    ]))
}

/// 按宽窄模式格式化本地时间戳（宽屏 `YYYY-MM-DD HH:MM`，窄屏 `MM-DD HH:MM`）。
fn format_table_time(ts_ms: i64, wide: bool) -> String {
    let full = format_datetime(ts_ms);
    if wide || full.len() != 16 {
        full
    } else {
        full[5..].to_string()
    }
}

/// 截取会话 ID 前 `max_chars` 个字符。
fn short_id(id: &str, max_chars: usize) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= max_chars {
        id.to_string()
    } else {
        chars[..max_chars].iter().collect()
    }
}

/// 按终端显示宽度截断过长字符串并补省略号（兼容中英文混排）。
fn truncate_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// 格式化紧凑上下文大小（如 `162k`、`1.2M`），用于 `PeakCtx` 列。
fn format_compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", (tokens as f64 / 1_000.0).round() as u64)
    } else if tokens == 0 {
        "—".to_string()
    } else {
        tokens.to_string()
    }
}

/// 格式化轮次时间跨度（如 `42s`、`14m`、`2h15m`）。
fn format_span(secs: u64) -> String {
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    let rem_mins = mins % 60;
    if rem_mins == 0 {
        format!("{}h", hours)
    } else {
        format!("{}h{}m", hours, rem_mins)
    }
}
