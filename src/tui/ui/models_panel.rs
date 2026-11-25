//! 统一的模型明细面板组件（`agent · provider · model`）与跨表共享的动态数值列宽规范。
//!
//! 所有表格均按「表头宽度 vs 实际单元格最大宽度」动态计算右侧数值列宽，
//! 既杜绝固定列宽带来的多余空隙，又保证大数值不被截断；同时在双表视图中
//! 让上下两张表共用同一套右侧列宽，确保逐列严格垂直对齐。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row, Table, TableState};
use ratatui::Frame;
use token_counter::core::format::{
    format_cost, format_int, format_percent, format_speed, format_tokens_m, format_ttft,
};
use token_counter::stats::aggregate::BucketRow;

use super::style::{
    highlight_style, model_color, panel_block, render_scrollbar, table_header_style,
    table_rows_rect,
};
use crate::tui::app::TableHit;
use crate::tui::data::ModelRow;

/// 展开完整 token 细分列（`In / Out / Cache`）所需的最小宽度。
pub const WIDE_TABLE_THRESHOLD: u16 = 112;

/// 表格统一列间距（2 格，视觉宽松且不会产生空洞）。
pub const TABLE_COL_SPACING: u16 = 2;

/// 有界指标列的固定紧凑宽度（不因切换选中行而左右抖动；极端超界值仍自动保底扩展）。
const FIXED_CACHE_PCT_W: usize = 6; // "Cache%" / "100.0%"
const FIXED_TTFT_W: usize = 5; // "99.9s"
const FIXED_SPEED_W: usize = 5; // "Speed" / "999/s"
/// 无界量级列的稳定下限（覆盖 `<0.001M`、`99.999M`、`$99.999`，更大时按父表全集动态扩宽）。
const MIN_TOKENS_M_W: usize = 7;
const MIN_COST_W: usize = 7;

/// 按「有界列固定 + 无界列动态」策略计算指标表列宽（第 0 列弹性吸收剩余宽度）。
///
/// 双表视图只传上半主表全集即可（子行均为父行分项，总量必 `<=` 父行），
/// 从而保证上下两表逐列垂直对齐且**滚动切换分桶时列宽完全稳定零抖动**。
pub fn compute_metrics_widths<'a>(
    rows: impl IntoIterator<Item = &'a BucketRow>,
    wide: bool,
) -> Vec<Constraint> {
    if wide {
        let mut w_reqs = "Reqs".len();
        let mut w_in = MIN_TOKENS_M_W;
        let mut w_out = MIN_TOKENS_M_W;
        let mut w_cache = MIN_TOKENS_M_W;
        let mut w_tokens = MIN_TOKENS_M_W;
        let mut w_cost = MIN_COST_W;
        let mut w_ttft = FIXED_TTFT_W;
        let mut w_speed = FIXED_SPEED_W;

        for m in rows {
            let out_total = m.output.saturating_add(m.reasoning);
            w_reqs = w_reqs.max(format_int(m.requests).len());
            w_in = w_in.max(format_tokens_m(m.input).len());
            w_out = w_out.max(format_tokens_m(out_total).len());
            w_cache = w_cache.max(format_tokens_m(m.cache_tokens()).len());
            w_tokens = w_tokens.max(format_tokens_m(m.total_tokens()).len());
            w_cost = w_cost.max(format_cost(m.cost_usd).len());
            w_ttft = w_ttft.max(format_ttft(m.avg_ttft_secs()).len());
            w_speed = w_speed.max(format_speed(m.avg_speed_tps()).len());
        }

        vec![
            Constraint::Min(20),
            Constraint::Length(w_reqs as u16),
            Constraint::Length(w_in as u16),
            Constraint::Length(w_out as u16),
            Constraint::Length(w_cache as u16),
            Constraint::Length(w_tokens as u16),
            Constraint::Length(FIXED_CACHE_PCT_W as u16),
            Constraint::Length(w_cost as u16),
            Constraint::Length(w_ttft as u16),
            Constraint::Length(w_speed as u16),
        ]
    } else {
        let mut w_reqs = "Reqs".len();
        let mut w_tokens = MIN_TOKENS_M_W;
        let mut w_cost = MIN_COST_W;
        let mut w_ttft = FIXED_TTFT_W;
        let mut w_speed = FIXED_SPEED_W;

        for m in rows {
            w_reqs = w_reqs.max(format_int(m.requests).len());
            w_tokens = w_tokens.max(format_tokens_m(m.total_tokens()).len());
            w_cost = w_cost.max(format_cost(m.cost_usd).len());
            w_ttft = w_ttft.max(format_ttft(m.avg_ttft_secs()).len());
            w_speed = w_speed.max(format_speed(m.avg_speed_tps()).len());
        }

        vec![
            Constraint::Min(16),
            Constraint::Length(w_reqs as u16),
            Constraint::Length(w_tokens as u16),
            Constraint::Length(FIXED_CACHE_PCT_W as u16),
            Constraint::Length(w_cost as u16),
            Constraint::Length(w_ttft as u16),
            Constraint::Length(w_speed as u16),
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
            Cell::from(Line::from(format_percent(m.cache_percent())).right_aligned()),
            cost_cell,
            Cell::from(Line::from(format_ttft(m.avg_ttft_secs())).right_aligned()),
            speed_cell,
        ])
    } else {
        Row::new([
            first_cell,
            Cell::from(Line::from(format_int(m.requests)).right_aligned()),
            Cell::from(Line::from(format_tokens_m(m.total_tokens())).right_aligned()),
            Cell::from(Line::from(format_percent(m.cache_percent())).right_aligned()),
            cost_cell,
            Cell::from(Line::from(format_ttft(m.avg_ttft_secs())).right_aligned()),
            speed_cell,
        ])
    }
}

/// 渲染统一的模型面板（支持聚焦亮框、选中高亮、滚动条与可选外部共享列宽），返回鼠标点击热区。
pub fn render(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: &[ModelRow],
    shared_widths: Option<&[Constraint]>,
    table_state: &mut TableState,
    focused: bool,
) -> TableHit {
    let block = panel_block(title, focused);
    let inner = block.inner(area);
    let wide = area.width >= WIDE_TABLE_THRESHOLD;

    let header = metrics_table_header("Model (agent · provider · model)", wide);
    let owned_widths;
    let widths: &[Constraint] = match shared_widths {
        Some(w) => w,
        None => {
            owned_widths = compute_metrics_widths(items.iter().map(|i| &i.metrics), wide);
            &owned_widths
        }
    };

    let mut rows: Vec<Row> = Vec::with_capacity(items.len());
    for item in items {
        let color = model_color(&item.model);
        let name_line = Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::raw(item.display_name()),
        ]);
        rows.push(metrics_table_row(Cell::from(name_line), &item.metrics, wide));
    }

    let mut table = Table::new(rows, widths.to_vec())
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
