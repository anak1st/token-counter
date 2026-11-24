//! `Day` Tab 视图：上表按天倒序滚动列表 + 下表统一模型明细面板（按 token 降序）。
//!
//! 上下两表复用 `models_panel` 中同一套右侧数值列宽与间距，使每一列数字跨框垂直对齐。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{
    Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
};
use ratatui::Frame;

use super::models_panel::{
    self, metrics_table_header, metrics_table_row, metrics_table_widths, TABLE_COL_SPACING,
    WIDE_TABLE_THRESHOLD,
};
use super::{highlight_style, split_upper_and_models, styled_block};
use crate::tui::app::App;
use crate::tui::data::TuiSnapshot;

/// 渲染 `Day` 视图（上下联动双表）。
pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let snap = match app.snapshot.as_ref() {
        Some(s) => s.clone(),
        None => return,
    };

    if snap.daily_rows.is_empty() {
        let block = styled_block("Daily");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("No usage data found.").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let (upper_area, models_area) = split_upper_and_models(area);

    render_daily_table(frame, app, &snap, upper_area);

    if let Some((date, items)) = app.selected_day_breakdown() {
        let title = format!("Models · {} (by tokens)", date);
        models_panel::render(frame, models_area, &title, items, None);
    }
}

/// 上半区：倒序每日主表（支持 ↑/↓ 滚动选中）。
fn render_daily_table(frame: &mut Frame, app: &mut App, snap: &TuiSnapshot, area: Rect) {
    let title = format!("Daily ({} days)", snap.daily_rows.len());
    let block = styled_block(&title);
    let inner = block.inner(area);
    let wide = area.width >= WIDE_TABLE_THRESHOLD;

    let header = metrics_table_header("Date", wide);
    let widths = metrics_table_widths(wide);

    let mut rows: Vec<Row> = Vec::with_capacity(snap.daily_rows.len());
    for row in &snap.daily_rows {
        rows.push(metrics_table_row(Cell::from(row.label.as_str()), row, wide));
    }

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(TABLE_COL_SPACING)
        .row_highlight_style(highlight_style());

    frame.render_stateful_widget(table, area, &mut app.day_table);

    if snap.daily_rows.len() > inner.height.saturating_sub(1) as usize {
        let selected = app.day_table.selected().unwrap_or(0);
        let mut sb_state = ScrollbarState::new(snap.daily_rows.len()).position(selected);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb_state,
        );
    }
}
