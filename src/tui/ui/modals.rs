//! TUI 居中弹窗组件（Agent 筛选弹窗与未知模型列表弹窗）。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Row, Table};
use ratatui::Frame;
use token_counter::core::format::{format_cost, format_int, format_tokens_m};

use super::models_panel::TABLE_COL_SPACING;
use super::style::{
    centered_rect, highlight_style, panel_block, render_scrollbar, table_header_style,
    table_rows_rect,
};
use crate::tui::app::{App, TableHit};

/// 弹窗最大宽度。
const AGENT_MODAL_MAX_WIDTH: u16 = 68;
const UNKNOWN_MODAL_MAX_WIDTH: u16 = 66;

/// 渲染居中的 Agent 选择弹窗（覆盖在主视图之上），返回弹窗热区。
pub fn render_agent_picker(frame: &mut Frame, app: &mut App, area: Rect) -> Option<TableHit> {
    if area.width < 28 || area.height < 7 {
        return None;
    }

    let options = app.picker_options();
    if options.is_empty() {
        return None;
    }

    // 高度自适应：上下边框(2) + 表头(1) + 选项行数，并限制在终端高度内以支持滚动
    let desired_height = (options.len() as u16).saturating_add(3);
    let modal_height = desired_height.min(area.height.saturating_sub(4)).max(5);
    let modal_width = AGENT_MODAL_MAX_WIDTH.min(area.width.saturating_sub(4));
    let modal_area = centered_rect(modal_width, modal_height, area);

    frame.render_widget(Clear, modal_area);

    let footer_hint = " ↑/↓ select · Enter confirm · Esc cancel ";
    let block = panel_block("Select Agent", true).title_bottom(
        Line::from(Span::styled(footer_hint, Style::default().fg(Color::DarkGray))).centered(),
    );
    let inner = block.inner(modal_area);

    let header = Row::new([
        Cell::from("Agent"),
        Cell::from(Line::from("Sessions").right_aligned()),
        Cell::from(Line::from("Tokens").right_aligned()),
        Cell::from(Line::from("Cost").right_aligned()),
    ])
    .style(table_header_style());

    let mut w_sessions = "Sessions".len();
    let mut w_tokens = 7usize;
    let mut w_cost = 7usize;

    let mut rows: Vec<Row> = Vec::with_capacity(options.len());
    for &opt in &options {
        let is_active = opt == app.agent_filter;
        let marker = if is_active {
            Span::styled("● ", Style::default().fg(Color::Cyan))
        } else {
            Span::raw("  ")
        };

        let label_text = match opt {
            None => "all · All Agents".to_string(),
            Some(agent) => format!("{} · {}", agent.id(), agent.label()),
        };

        let (sessions_count, tokens, cost_usd) = match app.snapshot.as_ref() {
            Some(snap) => {
                let view = snap.view(opt);
                (
                    view.sessions.len() as u64,
                    view.overall_total.total_tokens(),
                    view.overall_total.cost_usd,
                )
            }
            None => (0, 0, 0.0),
        };

        let s_text = format_int(sessions_count);
        let t_text = format_tokens_m(tokens);
        let c_text = format_cost(cost_usd);
        w_sessions = w_sessions.max(s_text.len());
        w_tokens = w_tokens.max(t_text.len());
        w_cost = w_cost.max(c_text.len());

        let name_line = Line::from(vec![marker, Span::raw(label_text)]);
        let cost_cell = Cell::from(Line::from(c_text).right_aligned())
            .style(Style::default().fg(Color::Green));

        rows.push(Row::new([
            Cell::from(name_line),
            Cell::from(Line::from(s_text).right_aligned()),
            Cell::from(Line::from(t_text).right_aligned()),
            cost_cell,
        ]));
    }

    let widths = [
        Constraint::Min(22),
        Constraint::Length(w_sessions as u16),
        Constraint::Length(w_tokens as u16),
        Constraint::Length(w_cost as u16),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(TABLE_COL_SPACING)
        .row_highlight_style(highlight_style());

    frame.render_stateful_widget(table, modal_area, &mut app.modal_table);
    render_scrollbar(
        frame,
        modal_area,
        inner,
        options.len(),
        app.modal_table.selected(),
    );

    Some(TableHit {
        panel: modal_area,
        rows: Some(table_rows_rect(inner)),
    })
}

/// 渲染居中的未知模型弹窗，返回弹窗热区（点击外区自动关闭）。
pub fn render_unknown_models(frame: &mut Frame, app: &mut App, area: Rect) -> Option<TableHit> {
    if area.width < 28 || area.height < 7 {
        return None;
    }

    let items: Vec<String> = app
        .active_view()
        .map(|v| v.unknown_models.clone())
        .unwrap_or_default();
    if items.is_empty() {
        return None;
    }

    let desired_height = (items.len() as u16).saturating_add(3);
    let modal_height = desired_height.min(area.height.saturating_sub(4)).max(5);
    let modal_width = UNKNOWN_MODAL_MAX_WIDTH.min(area.width.saturating_sub(4));
    let modal_area = centered_rect(modal_width, modal_height, area);

    frame.render_widget(Clear, modal_area);

    let title = format!("Unknown Models ({})", items.len());
    let footer_hint = " ~/.config/token-counter/models.json · Esc close ";
    let block = panel_block(&title, true).title_bottom(
        Line::from(Span::styled(footer_hint, Style::default().fg(Color::DarkGray))).centered(),
    );
    let inner = block.inner(modal_area);

    let header = Row::new([Cell::from("Model (agent · provider · model)")]).style(table_header_style());
    let rows: Vec<Row> = items
        .iter()
        .map(|name| Row::new([Cell::from(name.as_str())]))
        .collect();

    let table = Table::new(rows, [Constraint::Min(20)])
        .header(header)
        .block(block)
        .row_highlight_style(highlight_style());

    frame.render_stateful_widget(table, modal_area, &mut app.modal_table);
    render_scrollbar(frame, modal_area, inner, items.len(), app.modal_table.selected());

    Some(TableHit {
        panel: modal_area,
        rows: Some(table_rows_rect(inner)),
    })
}
