//! `Session` Tab 视图：上半区会话列表 + 下半区逐个 Turn 明细。
//!
//! 布局、边框样式与列宽规范均与时间维度视图保持一致：
//! - 共用全局 `agent_filter`（支持 `All` 与任意单个 Agent）；
//! - 上下两表共同按实际数据量算右侧数值列宽，既紧凑无多余空隙，又逐列严格垂直对齐；
//! - `Turn` 列右对齐精确宽度紧贴 `Time`，彻底消除编号旁边的视觉空洞。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use token_counter::core::format::{
    format_compact_tokens, format_cost, format_percent, format_span, format_speed, format_tokens_m,
    format_ttft, short_id, truncate_width,
};
use token_counter::core::time::format_datetime;
use token_counter::stats::session::{SessionDetail, TurnSummary};
use unicode_width::UnicodeWidthStr;

use super::models_panel::TABLE_COL_SPACING;
use super::style::{
    highlight_style, model_color, panel_block, render_scrollbar, split_upper_and_models,
    styled_block, table_header_style, table_rows_rect, upper_highlight_style,
};
use crate::tui::app::{FocusPane, TableHit};

/// 展开完整列（含 `PeakCtx / Cache% / TTFT` 与完整年份时间）所需的最小终端宽度。
const WIDE_SESSION_THRESHOLD: u16 = 126;

/// 有界固定列宽常量（保证上下翻会话时整张表完全稳定不抖动）。
const FIXED_TURN_W: usize = 4; // "Turn" / "#999"
const FIXED_TURNS_SPAN_W: usize = 6; // "Turns" / "12h30m"
const FIXED_STEPS_W: usize = 5; // "Steps" / "99999"
const FIXED_PEAK_CTX_W: usize = 7; // "PeakCtx" / "999.9M"
const FIXED_CACHE_PCT_W: usize = 6; // "Cache%" / "100.0%"
const FIXED_TTFT_W: usize = 5; // "99.9s"
const FIXED_SPEED_W: usize = 5; // "Speed" / "999/s"
/// 无界量级列的稳定下限。
const MIN_TOKENS_M_W: usize = 7; // "<0.001M" / "99.999M"
const MIN_COST_W: usize = 7; // "$99.999"

/// 上下两表共用的列宽（有界列写死，无界量级列按会话全集动态扩展）。
struct SessionColWidths {
    time_w: u16,
    turn_w: u16,
    session_id_w: u16,
    folder_w: u16,
    turns_or_span_w: u16,
    steps_w: u16,
    peak_w: u16,
    tokens_w: u16,
    cache_w: u16,
    cost_w: u16,
    ttft_w: u16,
    speed_w: u16,
}

impl SessionColWidths {
    /// 仅按 `sessions` 全集计算列宽（有界列固定，无界列动态），确保切换选中会话时列宽零抖动。
    fn measure(sessions: &[SessionDetail], show_agent_prefix: bool, wide: bool) -> Self {
        let id_len = if wide { 12 } else { 8 };
        let folder_cap: usize = if wide { 20 } else { 14 };
        let mut session_id_w = "Session".len();
        let mut folder_w = "Folder".len();
        let mut turn_w = FIXED_TURN_W;
        let mut turns_or_span_w = FIXED_TURNS_SPAN_W;
        let mut steps_w = FIXED_STEPS_W;
        let mut tokens_w = MIN_TOKENS_M_W;
        let mut cost_w = MIN_COST_W;
        let mut ttft_w = FIXED_TTFT_W;
        let mut speed_w = FIXED_SPEED_W;

        for item in sessions {
            let s = &item.summary;
            let short = short_id(&s.session_id, id_len);
            let id_col_len = if show_agent_prefix {
                s.agent.id().len() + 3 + short.len()
            } else {
                short.len()
            };
            session_id_w = session_id_w.max(id_col_len);

            if let Some(ref folder) = s.folder {
                let capped = truncate_width(folder, folder_cap);
                folder_w = folder_w.max(UnicodeWidthStr::width(capped.as_str()));
            }

            turn_w = turn_w.max(format!("#{}", s.turns).len());
            turns_or_span_w = turns_or_span_w.max(s.turns.to_string().len());
            steps_w = steps_w.max(s.steps.to_string().len());
            tokens_w = tokens_w.max(format_tokens_m(s.total_tokens()).len());
            cost_w = cost_w.max(format_cost(s.cost_usd).len());
            ttft_w = ttft_w.max(format_ttft(s.avg_ttft_secs()).len());
            speed_w = speed_w.max(format_speed(s.avg_speed_tps()).len());
        }

        Self {
            time_w: if wide { 16 } else { 11 },
            turn_w: turn_w as u16,
            session_id_w: session_id_w as u16,
            folder_w: folder_w.min(folder_cap) as u16,
            turns_or_span_w: turns_or_span_w as u16,
            steps_w: steps_w as u16,
            peak_w: FIXED_PEAK_CTX_W as u16,
            tokens_w: tokens_w as u16,
            cache_w: FIXED_CACHE_PCT_W as u16,
            cost_w: cost_w as u16,
            ttft_w: ttft_w as u16,
            speed_w: speed_w as u16,
        }
    }

    /// 上半区 `Sessions` 列宽：`Session`（ID）、`Name`（弹性名称）、`Folder`（独立文件夹）分列展示，仅保留 `Turns`（不显示 `Model` 与 `Steps`）。
    fn sessions_constraints(&self, wide: bool) -> Vec<Constraint> {
        if wide {
            vec![
                Constraint::Length(self.time_w),
                Constraint::Length(self.session_id_w),
                Constraint::Min(14),
                Constraint::Length(self.folder_w),
                Constraint::Length(self.turns_or_span_w),
                Constraint::Length(self.peak_w),
                Constraint::Length(self.tokens_w),
                Constraint::Length(self.cache_w),
                Constraint::Length(self.cost_w),
                Constraint::Length(self.ttft_w),
                Constraint::Length(self.speed_w),
            ]
        } else {
            vec![
                Constraint::Length(self.time_w),
                Constraint::Length(self.session_id_w),
                Constraint::Min(12),
                Constraint::Length(self.folder_w),
                Constraint::Length(self.turns_or_span_w),
                Constraint::Length(self.tokens_w),
                Constraint::Length(self.cost_w),
                Constraint::Length(self.speed_w),
            ]
        }
    }

    /// 下半区 `Turns` 列宽：`Turn` 精确右对齐紧贴 `Time`、`Model` 弹性、右侧数值块与上表逐列对齐。
    fn turns_constraints(&self, wide: bool) -> Vec<Constraint> {
        if wide {
            vec![
                Constraint::Length(self.turn_w),
                Constraint::Length(self.time_w),
                Constraint::Min(18),
                Constraint::Length(self.turns_or_span_w),
                Constraint::Length(self.steps_w),
                Constraint::Length(self.peak_w),
                Constraint::Length(self.tokens_w),
                Constraint::Length(self.cache_w),
                Constraint::Length(self.cost_w),
                Constraint::Length(self.ttft_w),
                Constraint::Length(self.speed_w),
            ]
        } else {
            vec![
                Constraint::Length(self.turn_w),
                Constraint::Length(self.time_w),
                Constraint::Min(14),
                Constraint::Length(self.turns_or_span_w),
                Constraint::Length(self.steps_w),
                Constraint::Length(self.tokens_w),
                Constraint::Length(self.cost_w),
                Constraint::Length(self.speed_w),
            ]
        }
    }
}

/// 渲染 `Session` Tab，返回 `(上表热区, 下表热区)`。
pub fn render(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionDetail],
    show_agent_prefix: bool,
    session_table: &mut TableState,
    turn_table: &mut TableState,
    focus: FocusPane,
) -> (TableHit, TableHit) {
    let (upper_area, lower_area) = split_upper_and_models(area);
    let wide = area.width >= WIDE_SESSION_THRESHOLD;
    let selected_detail = session_table.selected().and_then(|idx| sessions.get(idx));
    let widths = SessionColWidths::measure(sessions, show_agent_prefix, wide);

    let upper_hit = render_sessions_table(
        frame,
        upper_area,
        sessions,
        show_agent_prefix,
        wide,
        &widths,
        session_table,
        focus == FocusPane::Upper,
    );

    let lower_hit = render_turns_table(
        frame,
        lower_area,
        selected_detail,
        show_agent_prefix,
        wide,
        &widths,
        turn_table,
        focus == FocusPane::Lower,
    );

    (upper_hit, lower_hit)
}

/// 上半区：会话列表（按最后活跃时间倒序）。
#[allow(clippy::too_many_arguments)]
fn render_sessions_table(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionDetail],
    show_agent_prefix: bool,
    wide: bool,
    widths: &SessionColWidths,
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
    let header = sessions_header(wide);
    let constraints = widths.sessions_constraints(wide);
    let id_len = if wide { 12 } else { 8 };
    let folder_budget = widths.folder_w as usize;

    let mut rows: Vec<Row> = Vec::with_capacity(sessions.len());
    for detail in sessions {
        let s = &detail.summary;
        let id_short = short_id(&s.session_id, id_len);
        let session_cell = if show_agent_prefix {
            Cell::from(Line::from(vec![
                Span::styled(s.agent.id(), Style::default().fg(Color::DarkGray)),
                Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                Span::raw(id_short),
            ]))
        } else {
            Cell::from(id_short)
        };

        let name_cell = match s.name.as_deref() {
            Some(name) => Cell::from(name.to_string()),
            None => Cell::from("—").style(Style::default().fg(Color::DarkGray)),
        };
        let folder_cell = match s.folder.as_deref() {
            Some(folder) => Cell::from(truncate_width(folder, folder_budget)),
            None => Cell::from("—").style(Style::default().fg(Color::DarkGray)),
        };

        let time_text = format_table_time(s.last_ts_ms, wide);
        let cost_cell = Cell::from(Line::from(format_cost(s.cost_usd)).right_aligned())
            .style(Style::default().fg(Color::Green));
        let speed_cell = Cell::from(Line::from(format_speed(s.avg_speed_tps())).right_aligned())
            .style(Style::default().fg(Color::LightCyan));

        if wide {
            rows.push(Row::new([
                Cell::from(time_text).style(Style::default().fg(Color::DarkGray)),
                session_cell,
                name_cell,
                folder_cell,
                Cell::from(Line::from(s.turns.to_string()).right_aligned()),
                Cell::from(Line::from(format_compact_tokens(s.peak_ctx)).right_aligned()),
                Cell::from(Line::from(format_tokens_m(s.total_tokens())).right_aligned()),
                Cell::from(Line::from(format_percent(s.cache_percent())).right_aligned()),
                cost_cell,
                Cell::from(Line::from(format_ttft(s.avg_ttft_secs())).right_aligned()),
                speed_cell,
            ]));
        } else {
            rows.push(Row::new([
                Cell::from(time_text).style(Style::default().fg(Color::DarkGray)),
                session_cell,
                name_cell,
                folder_cell,
                Cell::from(Line::from(s.turns.to_string()).right_aligned()),
                Cell::from(Line::from(format_tokens_m(s.total_tokens())).right_aligned()),
                cost_cell,
                speed_cell,
            ]));
        }
    }

    let table = Table::new(rows, constraints)
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

/// 下半区：当前选中会话的逐个 Turn 轮次明细（最新轮次排最前）。
#[allow(clippy::too_many_arguments)]
fn render_turns_table(
    frame: &mut Frame,
    area: Rect,
    detail: Option<&SessionDetail>,
    show_agent_prefix: bool,
    wide: bool,
    widths: &SessionColWidths,
    table_state: &mut TableState,
    focused: bool,
) -> TableHit {
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
            return TableHit {
                panel: area,
                rows: None,
            };
        }
    };

    let s = &detail.summary;
    let id_short = short_id(&s.session_id, 12);
    let prefix = if show_agent_prefix {
        format!("{} · {}", s.agent.id(), id_short)
    } else {
        id_short
    };
    let title_meta = s.name.as_deref().or(s.folder.as_deref());
    let title = match title_meta {
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
    let header = turns_header(wide);
    let constraints = widths.turns_constraints(wide);

    let mut rows: Vec<Row> = Vec::with_capacity(detail.turns.len());
    for turn in &detail.turns {
        rows.push(build_turn_row(turn, wide));
    }

    let mut table = Table::new(rows, constraints)
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

/// 构造单条 Turn 明细行（`Turn` 列右对齐，紧贴 `Time` 无多余空白）。
fn build_turn_row(turn: &TurnSummary, wide: bool) -> Row<'static> {
    let turn_cell = Cell::from(Line::from(format!("#{}", turn.index)).right_aligned());
    let time_text = format_table_time(turn.first_ts_ms, wide);
    let model_cell = format_model_cell(&turn.models);
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
            Cell::from(Line::from(format_percent(turn.cache_percent())).right_aligned()),
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

fn sessions_header(wide: bool) -> Row<'static> {
    if wide {
        Row::new([
            Cell::from("Time"),
            Cell::from("Session"),
            Cell::from("Name"),
            Cell::from("Folder"),
            Cell::from(Line::from("Turns").right_aligned()),
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
            Cell::from("Time"),
            Cell::from("Session"),
            Cell::from("Name"),
            Cell::from("Folder"),
            Cell::from(Line::from("Turns").right_aligned()),
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
            Cell::from(Line::from("Turn").right_aligned()),
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
            Cell::from(Line::from("Turn").right_aligned()),
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

/// 构造带彩色圆点的模型单元格（多模型按各自圆点串联展示，不折叠成 `+N`）。
fn format_model_cell(models: &[String]) -> Cell<'static> {
    if models.is_empty() {
        return Cell::from("unknown").style(Style::default().fg(Color::DarkGray));
    }

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(models.len() * 3);
    for (idx, model) in models.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled(", ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled("● ", Style::default().fg(model_color(model))));
        spans.push(Span::raw(model.clone()));
    }
    Cell::from(Line::from(spans))
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
