//! `Overview` Tab 视图：
//! - 顶部（3 行）：左侧精简全量汇总（`Days │ Reqs │ Tokens │ Cost`）+ 右侧 `Days │ Hours [t]` 切换按钮；
//! - 中部（弹性）：左右对称留白、支持横向滚动的堆叠柱状图（聚焦时白框高亮，滚轮横向滚动）；
//! - 底部（40%）：时间范围与柱状图当前可视窗口实时联动的统一 `Models` 面板。

use std::collections::{BTreeMap, HashMap};

use jiff::civil;
use jiff::{ToSpan, ZonedDateTime};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, TableState};
use ratatui::Frame;
use token_counter::core::format::{format_cost, format_int, format_tokens_m};
use token_counter::core::model::Agent;
use token_counter::report::aggregate::BucketRow;

use super::models_panel;
use super::style::{
    format_axis_tokens, model_color, nice_axis_ceil, panel_block, split_upper_and_models,
    styled_block,
};
use crate::tui::app::{ChartMode, FocusPane, TableHit};
use crate::tui::data::{ModelRow, PeriodData, ViewSnapshot};

/// 柱状图单根柱宽（2 列）与相邻柱间距（1 列），步长 3 列。
const BAR_WIDTH: usize = 2;
const BAR_GAP: usize = 1;
const BAR_PITCH: usize = BAR_WIDTH + BAR_GAP;
/// Y 轴刻度文本宽度（如 `" 200M"`、`" 400k"`、`"    0"`）。
const Y_LABEL_WIDTH: usize = 5;

/// `Overview` 渲染后回传的鼠标热区与当前窗口元数据。
pub struct OverviewHits {
    pub lower: TableHit,
    pub chart_panel: Rect,
    pub chart_days: Option<Rect>,
    pub chart_hours: Option<Rect>,
    pub chart_prev: Option<Rect>,
    pub chart_next: Option<Rect>,
    pub chart_page_step: usize,
    pub chart_max_offset: usize,
    pub window_models_len: usize,
}

/// 堆叠柱状图中单根柱子的单个模型分段。
struct ChartSegment {
    model: String,
    tokens: u64,
}

/// 堆叠柱状图中的单根柱子（按时间从旧到新排列）。
struct ChartBar {
    key: String,
    short_label: String,
    total_tokens: u64,
    segments: Vec<ChartSegment>,
}

/// 渲染 `Overview` 纵览视图，返回各交互热区与可视窗口同步结果。
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    area: Rect,
    view: &ViewSnapshot,
    is_partial: bool,
    chart_mode: ChartMode,
    chart_offset: usize,
    focus: FocusPane,
    table_state: &mut TableState,
) -> OverviewHits {
    let (upper_area, models_area) = split_upper_and_models(area);
    let upper_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(upper_area);

    let (chart_days, chart_hours) =
        render_summary_bar(frame, view, is_partial, chart_mode, upper_chunks[0]);

    let chart_focused = focus == FocusPane::Upper;
    let chart_result = render_stacked_bar_chart(
        frame,
        view,
        chart_mode,
        chart_offset,
        chart_focused,
        upper_chunks[1],
    );

    let window_models = aggregate_window_models(
        period_for_mode(view, chart_mode),
        &chart_result.visible_keys,
    );
    let models_title = match &chart_result.range_label {
        Some(range) => format!("Models · {} (by tokens)", range),
        None => "Models (by tokens)".to_string(),
    };
    let window_models_len = window_models.len();
    let lower = models_panel::render(
        frame,
        models_area,
        &models_title,
        &window_models,
        table_state,
        focus == FocusPane::Lower,
    );

    OverviewHits {
        lower,
        chart_panel: upper_chunks[1],
        chart_days,
        chart_hours,
        chart_prev: chart_result.prev_rect,
        chart_next: chart_result.next_rect,
        chart_page_step: chart_result.page_step,
        chart_max_offset: chart_result.max_offset,
        window_models_len,
    }
}

/// 顶部一行摘要栏：左侧精简全量汇总（`Days │ Reqs │ Tokens │ Cost`），右侧 `Days │ Hours [t]` 按钮。
fn render_summary_bar(
    frame: &mut Frame,
    view: &ViewSnapshot,
    is_partial: bool,
    mode: ChartMode,
    area: Rect,
) -> (Option<Rect>, Option<Rect>) {
    let title = if is_partial {
        "Last 30 Days Summary (loading history...)"
    } else {
        "All Time Summary"
    };
    let block = styled_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let active_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let inactive_style = Style::default().fg(Color::DarkGray);
    let (days_style, hours_style) = match mode {
        ChartMode::Days => (active_style, inactive_style),
        ChartMode::Hours => (inactive_style, active_style),
    };

    let toggle_line = Line::from(vec![
        Span::styled("Days", days_style),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled("Hours", hours_style),
        Span::styled(" [t] ", Style::default().fg(Color::Cyan)),
    ]);

    let toggle_w = (toggle_line.width() as u16).min(inner.width / 2);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(toggle_w)])
        .split(inner);

    let total = &view.overall_total;
    let mut items = vec![
        ("Days ", view.daily.rows.len().to_string(), Color::White),
        ("Reqs ", format_int(total.requests), Color::White),
        ("Tokens ", format_tokens_m(total.total_tokens()), Color::Cyan),
        ("Cost ", format_cost(total.cost_usd), Color::Green),
    ];

    // 极窄终端自动省去前两项次要计数，确保 Tokens 与 Cost 完整显示
    if chunks[0].width < 58 {
        items.drain(0..2);
    }

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (idx, (label, value, color)) in items.into_iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  │  ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            value,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);
    frame.render_widget(Paragraph::new(toggle_line), chunks[1]);

    if chunks[1].width >= 12 {
        let days_rect = Rect::new(chunks[1].x, chunks[1].y, 4, 1);
        let hours_rect = Rect::new(chunks[1].x + 7, chunks[1].y, 5, 1);
        (Some(days_rect), Some(hours_rect))
    } else {
        (None, None)
    }
}

/// 柱状图渲染结果，供下方 `Models` 联动聚合与翻页按钮命中计算使用。
struct ChartRenderResult {
    visible_keys: Vec<String>,
    range_label: Option<String>,
    prev_rect: Option<Rect>,
    next_rect: Option<Rect>,
    page_step: usize,
    max_offset: usize,
}

/// 中部左右对称的堆叠柱状图：内区左上角 `[< Prev]` 与右上角 `[Next >]` 控制时间范围移动。
fn render_stacked_bar_chart(
    frame: &mut Frame,
    view: &ViewSnapshot,
    mode: ChartMode,
    chart_offset: usize,
    focused: bool,
    area: Rect,
) -> ChartRenderResult {
    // 水平固定结构：[左外留白>=1] + [Y刻度(5)] + " │ "(3) + [柱群(3N-1)] + [轴尾留白(1)] + [右外留白>=1]
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let chrome_w = Y_LABEL_WIDTH + 3 + 1;
    let avail_bars_w = inner_w.saturating_sub(chrome_w + 2);
    let bar_count = if avail_bars_w >= BAR_WIDTH {
        (avail_bars_w + BAR_GAP) / BAR_PITCH
    } else {
        0
    };

    if bar_count == 0 || inner_h < 4 {
        let block = panel_block("Tokens", focused);
        frame.render_widget(block, area);
        return ChartRenderResult {
            visible_keys: Vec::new(),
            range_label: None,
            prev_rect: None,
            next_rect: None,
            page_step: 1,
            max_offset: 0,
        };
    }

    let now = ZonedDateTime::now();
    let total_span = total_history_span(&now, view, mode);
    let max_offset = total_span.saturating_sub(bar_count);
    let effective_offset = chart_offset.min(max_offset);
    let page_step = (bar_count / 2).max(1);

    let bars = match mode {
        ChartMode::Days => build_daily_bars(&now, &view.daily, bar_count, effective_offset),
        ChartMode::Hours => build_hourly_bars(&now, &view.hourly, bar_count, effective_offset),
    };

    let range_label = match (bars.first(), bars.last()) {
        (Some(first), Some(last)) => Some(format!("{} .. {}", first.short_label, last.short_label)),
        _ => None,
    };
    let window_header = match (bars.first(), bars.last()) {
        (Some(first), Some(last)) => match mode {
            ChartMode::Days => format!("{} .. {}", first.key, last.key),
            ChartMode::Hours => format!("{} .. {}", first.short_label, last.short_label),
        },
        _ => String::new(),
    };
    let unit_label = match mode {
        ChartMode::Days => "Days",
        ChartMode::Hours => "Hours",
    };
    let title = if total_span > bars.len() {
        format!("Tokens ({} / {} {})", bars.len(), total_span, unit_label)
    } else {
        format!("Tokens ({} {})", bars.len(), unit_label)
    };

    let block = panel_block(&title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_keys: Vec<String> = bars.iter().map(|b| b.key.clone()).collect();

    // 框内首行：左 `[< Prev]` + 居中当前时间窗口 + 右 `[Next >]`，与下方图表之间空 1 行
    let (plot_top_y, prev_rect, next_rect) = if inner.height >= 6 && inner.width >= 24 {
        let (p_rect, n_rect) = render_nav_bar(
            frame,
            inner,
            &window_header,
            effective_offset < max_offset,
            effective_offset > 0,
        );
        (inner.y + 2, p_rect, n_rect)
    } else if inner.height >= 5 && inner.width >= 24 {
        let (p_rect, n_rect) = render_nav_bar(
            frame,
            inner,
            &window_header,
            effective_offset < max_offset,
            effective_offset > 0,
        );
        (inner.y + 1, p_rect, n_rect)
    } else {
        (inner.y, None, None)
    };

    let avail_h = inner.bottom().saturating_sub(plot_top_y) as usize;
    let chart_height = avail_h.saturating_sub(2);
    if bars.is_empty() || chart_height == 0 {
        return ChartRenderResult {
            visible_keys,
            range_label,
            prev_rect,
            next_rect,
            page_step,
            max_offset,
        };
    }

    // 计算严格左右对称的水平偏移，消除视觉偏左感
    let bars_span = bars.len() * BAR_PITCH - BAR_GAP;
    let graphic_w = chrome_w + bars_span;
    let left_pad = inner_w.saturating_sub(graphic_w) / 2;

    let y_label_x = inner.x + left_pad as u16;
    let axis_x = y_label_x + Y_LABEL_WIDTH as u16 + 1;
    let plot_x = axis_x + 2;
    let axis_line_len = bars_span + 2; // 轴内左右各 1 列对称留白

    let raw_max = bars.iter().map(|b| b.total_tokens).max().unwrap_or(0);
    let axis_max = nice_axis_ceil(raw_max);
    let scale_max = axis_max.max(1);

    let buf = frame.buffer_mut();

    // 绘制 Y 轴（1 位有效数字整刻度）与纵轴线
    let max_label = format!(
        "{:>width$}",
        format_axis_tokens(axis_max),
        width = Y_LABEL_WIDTH
    );
    buf.set_string(
        y_label_x,
        plot_top_y,
        max_label,
        Style::default().fg(Color::DarkGray),
    );

    let zero_label = format!("{:>width$}", "0", width = Y_LABEL_WIDTH);
    buf.set_string(
        y_label_x,
        plot_top_y + chart_height as u16 - 1,
        zero_label,
        Style::default().fg(Color::DarkGray),
    );

    for row in 0..chart_height {
        buf.set_string(
            axis_x,
            plot_top_y + row as u16,
            "│",
            Style::default().fg(Color::DarkGray),
        );
    }

    // 逐柱绘制按模型着色的堆叠柱子
    for (i, bar) in bars.iter().enumerate() {
        if bar.total_tokens == 0 {
            continue;
        }
        let col_offset = i * BAR_PITCH;
        let mut cumulative = 0u64;
        let mut prev_row_top = 0usize;

        for seg in &bar.segments {
            cumulative = cumulative.saturating_add(seg.tokens);
            let target_top =
                ((cumulative as f64 / scale_max as f64) * chart_height as f64).round() as usize;
            let mut row_top = target_top.min(chart_height);
            if row_top == 0 && cumulative == bar.total_tokens {
                row_top = 1;
            }
            if row_top <= prev_row_top {
                continue;
            }

            let style = Style::default().fg(model_color(&seg.model));
            for h in prev_row_top..row_top {
                let y = plot_top_y + (chart_height - 1 - h) as u16;
                for c in 0..BAR_WIDTH {
                    let x = plot_x + (col_offset + c) as u16;
                    if x < inner.right() && y < inner.bottom() {
                        buf.set_string(x, y, "█", style);
                    }
                }
            }
            prev_row_top = row_top;
        }
    }

    // 绘制底部横轴线与均匀分布的时间刻度
    let axis_y = plot_top_y + chart_height as u16;
    buf.set_string(axis_x, axis_y, "└", Style::default().fg(Color::DarkGray));
    for c in 0..axis_line_len {
        let x = axis_x + 1 + c as u16;
        if x < inner.right() {
            buf.set_string(x, axis_y, "─", Style::default().fg(Color::DarkGray));
        }
    }

    let label_y = axis_y + 1;
    if label_y < inner.bottom() {
        let last_idx = bars.len() - 1;
        let tick_indices = [0, bars.len() / 3, (bars.len() * 2) / 3, last_idx];
        let plot_right = plot_x + bars_span as u16;
        let mut last_end = 0u16;
        for &idx in &tick_indices {
            let label = &bars[idx].short_label;
            let label_len = label.len() as u16;
            let mut x = plot_x + (idx * BAR_PITCH) as u16;
            if x + label_len > plot_right {
                x = plot_right.saturating_sub(label_len);
            }
            if x >= last_end {
                buf.set_string(x, label_y, label, Style::default().fg(Color::DarkGray));
                last_end = x + label_len + 2;
            }
        }
    }

    ChartRenderResult {
        visible_keys,
        range_label,
        prev_rect,
        next_rect,
        page_step,
        max_offset,
    }
}

/// 在柱状图框内首行绘制：左 `[< Prev]` + 居中当前时间窗口 + 右 `[Next >]`。
fn render_nav_bar(
    frame: &mut Frame,
    inner: Rect,
    window_label: &str,
    can_prev: bool,
    can_next: bool,
) -> (Option<Rect>, Option<Rect>) {
    let prev_text = "[< Prev]";
    let next_text = "[Next >]";
    let prev_w = prev_text.len() as u16;
    let next_w = next_text.len() as u16;

    let prev_style = if can_prev {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let next_style = if can_next {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let prev_rect = Rect::new(inner.x + 1, inner.y, prev_w, 1);
    let next_rect = Rect::new(inner.right().saturating_sub(next_w + 1), inner.y, next_w, 1);

    // 两个按钮之间的居中区域显示当前时间窗口
    let center_x = prev_rect.right().saturating_add(1);
    let center_w = next_rect.x.saturating_sub(center_x.saturating_add(1));
    if center_w > 0 && !window_label.is_empty() {
        let center_rect = Rect::new(center_x, inner.y, center_w, 1);
        frame.render_widget(
            Paragraph::new(Span::styled(
                window_label,
                Style::default().fg(Color::White),
            ))
            .alignment(Alignment::Center),
            center_rect,
        );
    }

    frame.render_widget(Paragraph::new(Span::styled(prev_text, prev_style)), prev_rect);
    frame.render_widget(Paragraph::new(Span::styled(next_text, next_style)), next_rect);

    (Some(prev_rect), Some(next_rect))
}

fn period_for_mode(view: &ViewSnapshot, mode: ChartMode) -> &PeriodData {
    match mode {
        ChartMode::Days => &view.daily,
        ChartMode::Hours => &view.hourly,
    }
}

/// 合并柱状图当前可视时间窗口内的各模型指标，按总 token 降序返回。
fn aggregate_window_models(period: &PeriodData, visible_keys: &[String]) -> Vec<ModelRow> {
    let mut merged: HashMap<(Agent, String, String), BucketRow> = HashMap::new();

    for key in visible_keys {
        let items = match period.breakdowns.get(key) {
            Some(v) => v,
            None => continue,
        };
        for item in items {
            let group_key = (item.agent, item.provider.clone(), item.model.clone());
            let acc = merged
                .entry(group_key)
                .or_insert_with(|| BucketRow::empty(""));
            acc.fold(&item.metrics);
        }
    }

    let mut rows: Vec<ModelRow> = merged
        .into_iter()
        .map(|((agent, provider, model), metrics)| ModelRow {
            agent,
            provider,
            model,
            metrics,
        })
        .collect();

    rows.sort_by(|a, b| {
        b.metrics
            .total_tokens()
            .cmp(&a.metrics.total_tokens())
            .then_with(|| a.display_name().cmp(&b.display_name()))
    });
    rows
}

/// 计算当前视角从最早活跃分桶到当前时刻（含两端）的总连续分桶数，用于横向滚动上限。
fn total_history_span(now: &ZonedDateTime, view: &ViewSnapshot, mode: ChartMode) -> usize {
    match mode {
        ChartMode::Days => {
            let oldest_label = match view.daily.rows.last() {
                Some(r) => r.label.as_str(),
                None => return 0,
            };
            let oldest = match parse_date_key(oldest_label) {
                Some(d) => d,
                None => return 0,
            };
            let today = now.date();
            let days_diff = oldest.until(today).map(|s| s.get_days()).unwrap_or(0);
            if days_diff < 0 {
                1
            } else {
                days_diff as usize + 1
            }
        }
        ChartMode::Hours => {
            let oldest_label = match view.hourly.rows.last() {
                Some(r) => r.label.as_str(),
                None => return 0,
            };
            let oldest = match parse_hour_key(oldest_label) {
                Some(dt) => dt,
                None => return 0,
            };
            let current_hour = now.date().at(now.hour(), 0, 0, 0);
            let span = oldest.until(current_hour);
            match span {
                Ok(s) => {
                    let total_hours =
                        s.get_days() as i64 * 24 + s.get_hours() as i64;
                    if total_hours < 0 {
                        1
                    } else {
                        total_hours as usize + 1
                    }
                }
                Err(_) => 1,
            }
        }
    }
}

/// 解析 `YYYY-MM-DD` 日分桶键。
fn parse_date_key(label: &str) -> Option<civil::Date> {
    let mut parts = label.split('-');
    let y: i16 = parts.next()?.parse().ok()?;
    let m: i8 = parts.next()?.parse().ok()?;
    let d: i8 = parts.next()?.parse().ok()?;
    civil::Date::new(y, m, d).ok()
}

/// 解析 `YYYY-MM-DD HH:00` 小时分桶键。
fn parse_hour_key(label: &str) -> Option<civil::DateTime> {
    let (date_part, time_part) = label.split_once(' ')?;
    let date = parse_date_key(date_part)?;
    let hour: i8 = time_part.split(':').next()?.parse().ok()?;
    civil::DateTime::new(date.year(), date.month(), date.day(), hour, 0, 0, 0).ok()
}

/// 构造以 `today - offset` 为右端点的 `count` 个连续日历日柱子（左旧右新，空白日补 0）。
fn build_daily_bars(
    now: &ZonedDateTime,
    daily: &PeriodData,
    count: usize,
    offset: usize,
) -> Vec<ChartBar> {
    let today = now.date();
    let mut bars = Vec::with_capacity(count);

    for step in (0..count).rev() {
        let back = (offset + step) as i64;
        let date = match today.checked_sub(back.days()) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let key = format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day());
        let short_label = format!("{:02}-{:02}", date.month(), date.day());
        bars.push(build_bar_from_period(daily, key, short_label));
    }

    bars
}

/// 构造以 `current_hour - offset` 为右端点的 `count` 个连续本地整点柱子（左旧右新）。
fn build_hourly_bars(
    now: &ZonedDateTime,
    hourly: &PeriodData,
    count: usize,
    offset: usize,
) -> Vec<ChartBar> {
    let current_hour = match now
        .with()
        .minute(0)
        .second(0)
        .subsec_nanosecond(0)
        .build()
    {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };

    let mut bars = Vec::with_capacity(count);
    for step in (0..count).rev() {
        let back = (offset + step) as i64;
        let dt = match current_hour.checked_sub(back.hours()) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let date = dt.date();
        let key = format!(
            "{:04}-{:02}-{:02} {:02}:00",
            date.year(),
            date.month(),
            date.day(),
            dt.hour()
        );
        let short_label = format!("{:02}-{:02} {:02}h", date.month(), date.day(), dt.hour());
        bars.push(build_bar_from_period(hourly, key, short_label));
    }

    bars
}

/// 从 `PeriodData.breakdowns` 中汇总单根柱子各模型的分段用量。
fn build_bar_from_period(period: &PeriodData, key: String, short_label: String) -> ChartBar {
    let mut by_model: BTreeMap<&str, u64> = BTreeMap::new();
    let mut total_tokens = 0u64;

    if let Some(items) = period.breakdowns.get(&key) {
        for item in items {
            let tokens = item.metrics.total_tokens();
            if tokens > 0 {
                total_tokens = total_tokens.saturating_add(tokens);
                let entry = by_model.entry(item.model.as_str()).or_insert(0);
                *entry = entry.saturating_add(tokens);
            }
        }
    }

    let mut segments: Vec<ChartSegment> = by_model
        .into_iter()
        .map(|(model, tokens)| ChartSegment {
            model: model.to_string(),
            tokens,
        })
        .collect();

    // 分段按 token 升序排列，大头在顶部便于阅读
    segments.sort_by(|a, b| a.tokens.cmp(&b.tokens).then_with(|| a.model.cmp(&b.model)));

    ChartBar {
        key,
        short_label,
        total_tokens,
        segments,
    }
}
