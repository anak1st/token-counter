//! 统一的模型明细面板组件（`agent · provider · model`）与跨表共享的数值列宽规范。
//!
//! 被 `Overview`（全量汇总，可滚动选中）与 `Day`（选中日期明细，附当日 `Total` 行）共用，
//! 统一按总 token 使用量降序展示；同时向时间主表（`Day` 等）暴露同一套右侧数值列宽，
//! 确保上下两张表的每一列数字严格垂直对齐且间距均匀。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table, TableState};
use ratatui::Frame;
use token_counter::report::aggregate::BucketRow;

use super::{
    format_cache, format_cost, format_int, format_speed, format_tokens_m, format_ttft,
    highlight_style, model_color, panel_block, render_scrollbar, table_header_style,
    table_rows_rect,
};
use crate::tui::app::TableHit;
use crate::tui::data::ModelRow;

/// 展开完整 token 细分列（`In / Out / Cache`）所需的最小宽度。
pub const WIDE_TABLE_THRESHOLD: u16 = 112;

/// 表格统一列间距（2 格，视觉宽松且不会在末尾产生空洞）。
pub const TABLE_COL_SPACING: u16 = 2;

/// 构造共享的表格列宽：第 0 列吃掉剩余空间，右侧全部数值列在上下两表间严格垂直对齐。
pub fn metrics_table_widths(wide: bool) -> Vec<Constraint> {
    if wide {
        vec![
            Constraint::Min(24),
            Constraint::Length(6), // Reqs
            Constraint::Length(8), // In (M)
            Constraint::Length(8), // Out (M，含 reasoning)
            Constraint::Length(9), // Cache (M)
            Constraint::Length(9), // Tokens (M)
            Constraint::Length(7), // Cache%
            Constraint::Length(9), // Cost
            Constraint::Length(6), // TTFT
            Constraint::Length(6), // Speed
        ]
    } else {
        vec![
            Constraint::Min(20),
            Constraint::Length(6), // Reqs
            Constraint::Length(9), // Tokens (M)
            Constraint::Length(7), // Cache%
            Constraint::Length(9), // Cost
            Constraint::Length(6), // TTFT
            Constraint::Length(6), // Speed
        ]
    }
}

/// 构造共享的指标表头（第 0 列标题由调用方传入，如 `Date` 或 `Model (agent · provider · model)`）。
pub fn metrics_table_header(first_col: &'static str, wide: bool) -> Row<'static> {
    if wide {
        Row::new([
            Cell::from(first_col),
            Cell::from(Line::from("Reqs").right_aligned()),
            Cell::from(Line::from("In").right_aligned()),
            Cell::from(Line::from("Out").right_aligned()),
            Cell::from(Line::from("Cache").right_aligned()),
            Cell::from(Line::from("Tokens").right_aligned()),
            Cell::from(Line::from("Cache%").right_aligned()),
            Cell::from(Line::from("Cost").right_aligned()),
            Cell::from(Line::from("TTFT").right_aligned()),
            Cell::from(Line::from("Speed").right_aligned()),
        ])
        .style(table_header_style())
    } else {
        Row::new([
            Cell::from(first_col),
            Cell::from(Line::from("Reqs").right_aligned()),
            Cell::from(Line::from("Tokens").right_aligned()),
            Cell::from(Line::from("Cache%").right_aligned()),
            Cell::from(Line::from("Cost").right_aligned()),
            Cell::from(Line::from("TTFT").right_aligned()),
            Cell::from(Line::from("Speed").right_aligned()),
        ])
        .style(table_header_style())
    }
}

/// 构造共享的单行指标单元格（供时间主表与底部模型面板共用）。
pub fn metrics_table_row<'a>(first_cell: Cell<'a>, m: &BucketRow, wide: bool) -> Row<'a> {
    let cost_cell = Cell::from(Line::from(format_cost(m.cost_usd)).right_aligned())
        .style(Style::default().fg(Color::Green));
    let speed_cell = Cell::from(Line::from(format_speed(m.avg_speed_tps())).right_aligned())
        .style(Style::default().fg(Color::LightCyan));

    if wide {
        let out_total = m.output.saturating_add(m.reasoning);
        Row::new([
            first_cell,
            Cell::from(Line::from(format_int(m.requests)).right_aligned()),
            Cell::from(Line::from(format_tokens_m(m.input)).right_aligned()),
            Cell::from(Line::from(format_tokens_m(out_total)).right_aligned()),
            Cell::from(Line::from(format_tokens_m(m.cache_tokens())).right_aligned()),
            Cell::from(Line::from(format_tokens_m(m.total_tokens())).right_aligned()),
            Cell::from(Line::from(format_cache(m.cache_percent())).right_aligned()),
            cost_cell,
            Cell::from(Line::from(format_ttft(m.avg_ttft_secs())).right_aligned()),
            speed_cell,
        ])
    } else {
        Row::new([
            first_cell,
            Cell::from(Line::from(format_int(m.requests)).right_aligned()),
            Cell::from(Line::from(format_tokens_m(m.total_tokens())).right_aligned()),
            Cell::from(Line::from(format_cache(m.cache_percent())).right_aligned()),
            cost_cell,
            Cell::from(Line::from(format_ttft(m.avg_ttft_secs())).right_aligned()),
            speed_cell,
        ])
    }
}

/// 渲染统一的模型面板（支持聚焦亮框、选中高亮与滚动条），返回鼠标点击热区。
pub fn render(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: &[ModelRow],
    table_state: &mut TableState,
    focused: bool,
) -> TableHit {
    let block = panel_block(title, focused);
    let inner = block.inner(area);
    let wide = area.width >= WIDE_TABLE_THRESHOLD;

    let header = metrics_table_header("Model (agent · provider · model)", wide);
    let widths = metrics_table_widths(wide);

    let mut rows: Vec<Row> = Vec::with_capacity(items.len());
    for item in items {
        let color = model_color(&item.model);
        let name_line = Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::raw(item.display_name()),
        ]);
        rows.push(metrics_table_row(Cell::from(name_line), &item.metrics, wide));
    }

    let mut table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(TABLE_COL_SPACING);

    if focused {
        table = table.row_highlight_style(highlight_style());
    }

    frame.render_stateful_widget(table, area, table_state);
    render_scrollbar(frame, area, inner, items.len(), table_state.selected());

    TableHit {
        panel: area,
        rows: Some(table_rows_rect(inner)),
    }
}
