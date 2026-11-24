//! 通用时间维度 Tab 视图（共用一套上下联动双表渲染 `Hour` / `Day` / `Week` / `Month`）。
//!
//! 上半区 60%：按时间倒序的滚动主表（最新在最上面）；
//! 下半区 40%：当前选中时间分桶的统一 `Models` 面板（两表共用动态算出的右侧数值列宽，严格垂直对齐）。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use super::models_panel::{
    self, compute_metrics_widths, metrics_table_header, metrics_table_row, TABLE_COL_SPACING,
    WIDE_TABLE_THRESHOLD,
};
use super::style::{
    panel_block, render_scrollbar, split_upper_and_models, styled_block, table_rows_rect,
    upper_highlight_style,
};
use crate::tui::app::{App, FocusPane, TableHit};
use crate::tui::data::PeriodData;

/// 单个时间维度 Tab 的展示文案配置。
pub struct PeriodViewSpec {
    /// 上半区面板名称（如 `Hourly` / `Daily` / `Weekly` / `Monthly`）。
    pub panel_name: &'static str,
    /// 计数单位（如 `hours` / `days` / `weeks` / `months`）。
    pub unit_name: &'static str,
    /// 第 0 列表头（如 `Hour` / `Date` / `Week` / `Month`）。
    pub first_col: &'static str,
}

/// 渲染任意时间维度 Tab（`Hour` / `Day` / `Week` / `Month`），返回 `(上表热区, 下表热区)`。
pub fn render(
    frame: &mut Frame,
    area: Rect,
    spec: PeriodViewSpec,
    period: &PeriodData,
    table_state: &mut TableState,
    detail_table: &mut TableState,
    focus: FocusPane,
) -> (Option<TableHit>, Option<TableHit>) {
    if period.rows.is_empty() {
        let block = styled_block(spec.panel_name);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new("No usage data found.").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return (None, None);
    }

    let (upper_area, models_area) = split_upper_and_models(area);
    let wide = upper_area.width >= WIDE_TABLE_THRESHOLD;
    let breakdown = App::selected_period_breakdown(period, table_state);

    // 仅按上半主表全集计算共享列宽：子行均不超父行总量，上下翻行时整张表完全零抖动
    let shared_widths = compute_metrics_widths(period.rows.iter(), wide);

    let upper_hit = render_period_table(
        frame,
        upper_area,
        &spec,
        period,
        wide,
        &shared_widths,
        table_state,
        focus == FocusPane::Upper,
    );

    let lower_hit = breakdown.map(|(label, items)| {
        let title = format!("Models · {} (by tokens)", label);
        models_panel::render(
            frame,
            models_area,
            &title,
            items,
            Some(&shared_widths),
            detail_table,
            focus == FocusPane::Lower,
        )
    });

    (Some(upper_hit), lower_hit)
}

/// 上半区：时间倒序主表（聚焦时亮白框 + 青色高亮行，失焦时暗灰框保留选中行指示）。
#[allow(clippy::too_many_arguments)]
fn render_period_table(
    frame: &mut Frame,
    area: Rect,
    spec: &PeriodViewSpec,
    period: &PeriodData,
    wide: bool,
    widths: &[Constraint],
    table_state: &mut TableState,
    focused: bool,
) -> TableHit {
    let title = format!("{} ({} {})", spec.panel_name, period.rows.len(), spec.unit_name);
    let block = panel_block(&title, focused);
    let inner = block.inner(area);

    let header = metrics_table_header(spec.first_col, wide);

    let mut rows: Vec<Row> = Vec::with_capacity(period.rows.len());
    for row in &period.rows {
        rows.push(metrics_table_row(Cell::from(row.label.as_str()), row, wide));
    }

    let table = Table::new(rows, widths.to_vec())
        .header(header)
        .block(block)
        .column_spacing(TABLE_COL_SPACING)
        .row_highlight_style(upper_highlight_style(focused));

    frame.render_stateful_widget(table, area, table_state);
    render_scrollbar(frame, area, inner, period.rows.len(), table_state.selected());

    TableHit {
        panel: area,
        rows: Some(table_rows_rect(inner)),
    }
}
