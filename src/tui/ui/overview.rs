//! `Overview` Tab 视图：全量 KPI 摘要条 + 自适应宽度堆叠柱状图（支持 `Days` / `Hours`）+ 统一全量模型表。

use std::collections::BTreeMap;

use jiff::{ToSpan, ZonedDateTime};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, TableState};
use ratatui::Frame;

use super::{
    format_axis_tokens, format_cache, format_cost, format_int, format_speed, format_tokens_m,
    format_ttft, model_color, models_panel, nice_axis_ceil, split_upper_and_models, styled_block,
};
use crate::tui::app::{ChartMode, TableHit};
use crate::tui::data::{PeriodData, ViewSnapshot};

/// 柱状图单根柱宽（2 列）与相邻柱间距（1 列），步长 3 列。
const BAR_WIDTH: usize = 2;
const BAR_GAP: usize = 1;
const BAR_PITCH: usize = BAR_WIDTH + BAR_GAP;

/// `Overview` 渲染后回传的鼠标交互热区。
pub struct OverviewHits {
    pub lower: TableHit,
    pub chart_days: Option<Rect>,
    pub chart_hours: Option<Rect>,
}

/// 堆叠柱状图中单根柱子的单个模型分段。
struct ChartSegment {
    model: String,
    tokens: u64,
}

/// 堆叠柱状图中的单根柱子（按时间从旧到新排列）。
struct ChartBar {
    short_label: String,
    total_tokens: u64,
    segments: Vec<ChartSegment>,
}

/// 渲染 `Overview` 纵览视图，返回底部 `Models` 与柱状图 `Days/Hours` 切换热区。
pub fn render(
    frame: &mut Frame,
    area: Rect,
    view: &ViewSnapshot,
    is_partial: bool,
    chart_mode: ChartMode,
    table_state: &mut TableState,
) -> OverviewHits {
    let (upper_area, models_area) = split_upper_and_models(area);
    let upper_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(upper_area);

    render_kpi_bar(frame, view, is_partial, upper_chunks[0]);
    let (chart_days, chart_hours) =
        render_stacked_bar_chart(frame, view, chart_mode, upper_chunks[1]);

    let models_title = if is_partial {
        "Models · Last 30 Days (by tokens)"
    } else {
        "Models · All Time (by tokens)"
    };
    let lower = models_panel::render(
        frame,
        models_area,
        models_title,
        &view.model_rows,
        table_state,
        true,
    );

    OverviewHits {
        lower,
        chart_days,
        chart_hours,
    }
}

/// 顶部一行全量 KPI 摘要条：活跃天数、请求数、总 Tokens、总美元、缓存占比、平均 TTFT、平均纯解码速度。
fn render_kpi_bar(frame: &mut Frame, view: &ViewSnapshot, is_partial: bool, area: Rect) {
    let title = if is_partial {
        "Last 30 Days Summary (loading history...)"
    } else {
        "All Time Summary"
    };
    let block = styled_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total = &view.overall_total;
    let mut items = vec![
        ("Days ", view.daily.rows.len().to_string(), Color::White),
        ("Reqs ", format_int(total.requests), Color::White),
        ("Tokens ", format_tokens_m(total.total_tokens()), Color::Cyan),
        ("Cost ", format_cost(total.cost_usd), Color::Green),
        ("Cache ", format_cache(total.cache_percent()), Color::Yellow),
        ("TTFT ", format_ttft(total.avg_ttft_secs()), Color::Magenta),
        ("Speed ", format_speed(total.avg_speed_tps()), Color::LightCyan),
    ];

    // 窄终端自动省去前两项次要计数，确保核心 5 项完整居中不截断
    if inner.width < 95 {
        items.drain(0..2);
    }

    let mut spans: Vec<Span> = Vec::new();
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

    let paragraph = Paragraph::new(Line::from(spans)).alignment(Alignment::Center);
    frame.render_widget(paragraph, inner);
}

/// 中部自适应宽度堆叠柱状图：按终端可用宽度能放多少柱子就加载多少柱子，支持 `Days` / `Hours` 切换。
fn render_stacked_bar_chart(
    frame: &mut Frame,
    view: &ViewSnapshot,
    mode: ChartMode,
    area: Rect,
) -> (Option<Rect>, Option<Rect>) {
    let y_label_width: u16 = 6;
    let inner_width = area.width.saturating_sub(2);
    let inner_height = area.height.saturating_sub(2);
    let plot_width = inner_width.saturating_sub(y_label_width + 1) as usize;
    let bar_count = if plot_width >= BAR_WIDTH {
        (plot_width + BAR_GAP) / BAR_PITCH
    } else {
        0
    };

    let unit_label = match mode {
        ChartMode::Days => "Days",
        ChartMode::Hours => "Hours",
    };
    let title = if bar_count > 0 {
        format!("Tokens ({} {})", bar_count, unit_label)
    } else {
        "Tokens".to_string()
    };

    let block = styled_block(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if bar_count == 0 || inner.width < 16 || inner_height < 4 {
        return (None, None);
    }

    // 内区顶部留 1 行放框内 `Days │ Hours [t]` 切换按钮（极矮终端自动省略以保留绘图空间）
    let (plot_top_y, days_rect, hours_rect) = if inner.height >= 5 {
        let (d_rect, h_rect) = render_mode_toggle(frame, inner, mode);
        (inner.y + 1, d_rect, h_rect)
    } else {
        (inner.y, None, None)
    };

    let available_h = inner.bottom().saturating_sub(plot_top_y);
    let chart_height = available_h.saturating_sub(2) as usize;
    if chart_height == 0 {
        return (days_rect, hours_rect);
    }

    let now = ZonedDateTime::now();
    let bars = match mode {
        ChartMode::Days => build_daily_bars(&now, &view.daily, bar_count),
        ChartMode::Hours => build_hourly_bars(&now, &view.hourly, bar_count),
    };
    if bars.is_empty() {
        return (days_rect, hours_rect);
    }

    let raw_max = bars.iter().map(|b| b.total_tokens).max().unwrap_or(0);
    let axis_max = nice_axis_ceil(raw_max);
    let scale_max = axis_max.max(1);

    let buf = frame.buffer_mut();
    let plot_x = inner.x + y_label_width + 1;

    // 绘制 Y 轴（1 位有效数字整刻度）与纵轴线
    let max_label = format!(
        "{:>width$}",
        format_axis_tokens(axis_max),
        width = y_label_width as usize
    );
    buf.set_string(
        inner.x,
        plot_top_y,
        max_label,
        Style::default().fg(Color::DarkGray),
    );

    let zero_label = format!("{:>width$}", "0", width = y_label_width as usize);
    buf.set_string(
        inner.x,
        plot_top_y + chart_height as u16 - 1,
        zero_label,
        Style::default().fg(Color::DarkGray),
    );

    for row in 0..chart_height {
        buf.set_string(
            inner.x + y_label_width,
            plot_top_y + row as u16,
            "│",
            Style::default().fg(Color::DarkGray),
        );
    }

    // 逐柱绘制按模型着色的堆叠柱子（每根固定 BAR_WIDTH 列宽 + BAR_GAP 间距）
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
    buf.set_string(
        inner.x + y_label_width,
        axis_y,
        "└",
        Style::default().fg(Color::DarkGray),
    );
    for c in 0..plot_width {
        buf.set_string(
            plot_x + c as u16,
            axis_y,
            "─",
            Style::default().fg(Color::DarkGray),
        );
    }

    let label_y = axis_y + 1;
    if label_y < inner.bottom() {
        let last_idx = bars.len() - 1;
        let tick_indices = [0, bars.len() / 3, (bars.len() * 2) / 3, last_idx];
        let mut last_end = 0u16;
        for &idx in &tick_indices {
            let label = &bars[idx].short_label;
            let label_len = label.len() as u16;
            let mut x = plot_x + (idx * BAR_PITCH) as u16;
            if x + label_len > inner.right() {
                x = inner.right().saturating_sub(label_len);
            }
            if x >= last_end {
                buf.set_string(x, label_y, label, Style::default().fg(Color::DarkGray));
                last_end = x + label_len + 2;
            }
        }
    }

    (days_rect, hours_rect)
}

/// 在柱状图框内右上角绘制 `Days │ Hours [t]` 切换按钮，并返回各自点击热区。
fn render_mode_toggle(
    frame: &mut Frame,
    inner: Rect,
    mode: ChartMode,
) -> (Option<Rect>, Option<Rect>) {
    let active_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let inactive_style = Style::default().fg(Color::DarkGray);

    let (days_style, hours_style) = match mode {
        ChartMode::Days => (active_style, inactive_style),
        ChartMode::Hours => (inactive_style, active_style),
    };

    let line = Line::from(vec![
        Span::styled("Days", days_style),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled("Hours", hours_style),
        Span::styled(" [t] ", Style::default().fg(Color::Cyan)),
    ]);

    let total_w = line.width() as u16;
    if inner.width < total_w {
        return (None, None);
    }

    let start_x = inner.right().saturating_sub(total_w);
    let toggle_area = Rect::new(start_x, inner.y, total_w, 1);
    frame.render_widget(Paragraph::new(line), toggle_area);

    // 精确切分 `Days`（4 列）与 `Hours`（5 列，中间隔 3 列 ` │ `）的点击区
    let days_rect = Rect::new(start_x, inner.y, 4, 1);
    let hours_rect = Rect::new(start_x + 7, inner.y, 5, 1);
    (Some(days_rect), Some(hours_rect))
}

/// 构造最近 `count` 个连续日历日的柱子（左旧右新，空白日补 0）。
fn build_daily_bars(now: &ZonedDateTime, daily: &PeriodData, count: usize) -> Vec<ChartBar> {
    let today = now.date();
    let mut bars = Vec::with_capacity(count);

    for offset in (0..count).rev() {
        let date = match today.checked_sub((offset as i64).days()) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let key = format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day());
        let short_label = format!("{:02}-{:02}", date.month(), date.day());
        bars.push(build_bar_from_period(daily, &key, short_label));
    }

    bars
}

/// 构造最近 `count` 个连续本地整点的柱子（左旧右新，空白小时补 0）。
fn build_hourly_bars(now: &ZonedDateTime, hourly: &PeriodData, count: usize) -> Vec<ChartBar> {
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
    for offset in (0..count).rev() {
        let dt = match current_hour.checked_sub((offset as i64).hours()) {
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
        bars.push(build_bar_from_period(hourly, &key, short_label));
    }

    bars
}

/// 从 `PeriodData.breakdowns` 中汇总单根柱子各模型的分段用量。
fn build_bar_from_period(period: &PeriodData, key: &str, short_label: String) -> ChartBar {
    let mut by_model: BTreeMap<&str, u64> = BTreeMap::new();
    let mut total_tokens = 0u64;

    if let Some(items) = period.breakdowns.get(key) {
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
        short_label,
        total_tokens,
        segments,
    }
}
