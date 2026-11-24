//! `Session` Tab 视图：上半区会话列表 + 下半区逐个 Turn 明细。
//!
//! 布局、边框样式与全局筛选均与 `Hour` / `Day` / `Week` / `Month` 严格保持一致：
//! - 共用全局 `agent_filter`（支持 `All` 与任意单个 Agent）；
//! - 上半区 60% 为按最后活跃时间倒序排列的会话列表；
//! - 下半区 40% 为当前选中会话的逐个 Turn 明细（按时间升序，`Enter` 可切换焦点滚动）。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use token_counter::core::time::format_datetime;
use token_counter::report::session::TurnSummary;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::models_panel::TABLE_COL_SPACING;
use super::{
    format_cache, format_cost, format_speed, format_tokens_m, format_ttft, highlight_style,
    model_color, panel_block, render_scrollbar, split_upper_and_models, styled_block,
    table_header_style, table_rows_rect, upper_highlight_style,
};
use crate::tui::app::{FocusPane, TableHit};
use crate::tui::data::SessionItem;

/// 展开完整列（含 `PeakCtx / Cache% / TTFT` 与完整年份时间）所需的最小终端宽度。
const WIDE_SESSION_THRESHOLD: u16 = 126;

/// 渲染 `Session` Tab，返回 `(上表热区, 下表热区)`。
pub fn render(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionItem],
    show_agent_prefix: bool,
    session_table: &mut TableState,
    turn_table: &mut TableState,
    focus: FocusPane,
) -> (TableHit, TableHit) {
    let (upper_area, lower_area) = split_upper_and_models(area);

    let upper_hit = render_sessions_table(
        frame,
        upper_area,
        sessions,
        show_agent_prefix,
        session_table,
        focus == FocusPane::Upper,
    );

    let selected_item = session_table.selected().and_then(|idx| sessions.get(idx));
    let lower_hit = render_turns_table(
        frame,
        lower_area,
        selected_item,
        show_agent_prefix,
        turn_table,
        focus == FocusPane::Lower,
    );

    (upper_hit, lower_hit)
}

/// 上半区：会话列表（按最后活跃时间倒序）。
fn render_sessions_table(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionItem],
    show_agent_prefix: bool,
    table_state: &mut TableState,
    focused: bool,
) -> TableHit {
    let title = format!("Sessions ({})", sessions.len());
    let block = panel_block(&title, focused);

    if sessions.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("No sessions found.").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return TableHit {
            panel: area,
            rows: None,
        };
    }

    let inner = block.inner(area);
    let wide = area.width >= WIDE_SESSION_THRESHOLD;
    let header = sessions_header(wide);
    let widths = sessions_widths(wide);
    let model_budget = if wide { 16 } else { 14 };

    let mut rows: Vec<Row> = Vec::with_capacity(sessions.len());
    for item in sessions {
        let s = &item.detail.summary;
        let id_and_label = match s.label.as_deref() {
            Some(label) => format!("{} · {}", short_id(&s.session_id, 8), label),
            None => short_id(&s.session_id, 12),
        };
        let session_cell = if show_agent_prefix {
            Cell::from(Line::from(vec![
                Span::styled(item.agent.id(), Style::default().fg(Color::DarkGray)),
                Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                Span::raw(id_and_label),
            ]))
        } else {
            Cell::from(id_and_label)
        };

        let time_text = format_table_time(s.last_ts_ms, wide);
        let model_cell = format_model_cell(&s.models, model_budget);
        let cost_cell = Cell::from(Line::from(format_cost(s.cost_usd)).right_aligned())
            .style(Style::default().fg(Color::Green));
        let speed_cell = Cell::from(Line::from(format_speed(s.avg_speed_tps())).right_aligned())
            .style(Style::default().fg(Color::LightCyan));

        if wide {
            rows.push(Row::new([
                session_cell,
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
                session_cell,
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
        .row_highlight_style(upper_highlight_style(focused));

    frame.render_stateful_widget(table, area, table_state);
    render_scrollbar(frame, area, inner, sessions.len(), table_state.selected());

    TableHit {
        panel: area,
        rows: Some(table_rows_rect(inner)),
    }
}

/// 下半区：当前选中会话的逐个 Turn 轮次明细（按时间升序）。
fn render_turns_table(
    frame: &mut Frame,
    area: Rect,
    item: Option<&SessionItem>,
    show_agent_prefix: bool,
    table_state: &mut TableState,
    focused: bool,
) -> TableHit {
    let item = match item {
        Some(it) => it,
        None => {
            let block = styled_block("Turns");
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(
                Paragraph::new("Select a session to inspect turns.")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return TableHit {
                panel: area,
                rows: None,
            };
        }
    };

    let detail = &item.detail;
    let s = &detail.summary;
    let id_short = short_id(&s.session_id, 12);
    let prefix = if show_agent_prefix {
        format!("{} · {}", item.agent.id(), id_short)
    } else {
        id_short
    };
    let title = match s.label.as_deref() {
        Some(label) => format!(
            "Turns · {} · {} ({} turns)",
            prefix,
            truncate_width(label, 36),
            detail.turns.len()
        ),
        None => format!("Turns · {} ({} turns)", prefix, detail.turns.len()),
    };

    let block = panel_block(&title, focused);

    if detail.turns.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("No turns recorded.").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return TableHit {
            panel: area,
            rows: None,
        };
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
    render_scrollbar(frame, area, inner, detail.turns.len(), table_state.selected());

    TableHit {
        panel: area,
        rows: Some(table_rows_rect(inner)),
    }
}

/// 构造单条 Turn 明细行。
fn build_turn_row(turn: &TurnSummary, wide: bool) -> Row<'static> {
    let turn_cell = Cell::from(format!("#{}", turn.index));

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
            Constraint::Min(18),    // Session (id · label)
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
            Constraint::Min(14),    // Session
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
