//! 未知模型弹窗：仅简洁列出未命中价目表的 `agent · provider · model`，点击外部或按 `Esc` 关闭。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Row, Table};
use ratatui::Frame;

use super::style::{
    centered_rect, highlight_style, panel_block, render_scrollbar, table_header_style,
    table_rows_rect,
};
use crate::tui::app::{App, TableHit};

/// 弹窗最大宽度。
const MODAL_MAX_WIDTH: u16 = 66;

/// 渲染居中的未知模型弹窗，返回弹窗热区（点击外区自动关闭）。
pub fn render(frame: &mut Frame, app: &mut App, area: Rect) -> Option<TableHit> {
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
    let modal_width = MODAL_MAX_WIDTH.min(area.width.saturating_sub(4));
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
