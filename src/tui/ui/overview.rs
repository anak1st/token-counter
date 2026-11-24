//! `Overview` Tab 视图：核心 KPI 摘要条 + 近 30 天堆叠柱状图 + 统一全量模型表（按 token 降序）。

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{
    format_axis_tokens, format_cache, format_cost, format_int, format_speed, format_tokens_m,
    format_ttft, model_color, models_panel, split_upper_and_models, styled_block,
};
use crate::tui::app::App;
use crate::tui::data::{ChartDay, TuiSnapshot};

/// 渲染 `Overview` 纵览视图。
pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let snap = match app.snapshot.as_ref() {
        Some(s) => s.clone(),
        None => return,
    };

    let (upper_area, models_area) = split_upper_and_models(area);
    let upper_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(upper_area);

    render_kpi_bar(frame, &snap, upper_chunks[0]);
    render_stacked_bar_chart(frame, &snap.chart_days, upper_chunks[1]);
    let models_title = if snap.is_partial {
        "Models · Last 30 Days (by tokens)"
    } else {
        "Models · All Time (by tokens)"
    };
    models_panel::render(
        frame,
        models_area,
        models_title,
        &snap.model_rows,
        Some(&mut app.overview_table),
    );
}

/// 顶部一行 KPI 摘要条：活跃天数、请求数、总 Tokens、总美元、缓存占比、平均 TTFT、平均纯解码速度。
fn render_kpi_bar(frame: &mut Frame, snap: &TuiSnapshot, area: Rect) {
    let title = if snap.is_partial {
        "Last 30 Days Summary (loading history...)"
    } else {
        "All Time Summary"
    };
    let block = styled_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total = &snap.overall_total;
    let mut items = vec![
        ("Days ", snap.daily.rows.len().to_string(), Color::White),
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

/// 中部近 30 天按模型着色的堆叠柱状图（左旧右新，空白日保留间距）。
fn render_stacked_bar_chart(frame: &mut Frame, days: &[ChartDay], area: Rect) {
    let block = styled_block("Daily Tokens (Last 30 Days)");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if days.is_empty() || inner.width < 12 || inner.height < 4 {
        return;
    }

    let y_label_width: u16 = 7;
    let chart_width = inner.width.saturating_sub(y_label_width + 1) as usize;
    let chart_height = inner.height.saturating_sub(2) as usize;
    if chart_width == 0 || chart_height == 0 {
        return;
    }

    let max_tokens = days
        .iter()
        .map(|d| d.total_tokens)
        .max()
        .unwrap_or(0)
        .max(1);

    let buf = frame.buffer_mut();
    let bar_count = days.len();
    let plot_x = inner.x + y_label_width + 1;

    // 绘制 Y 轴最大/最小刻度与纵轴线
    let max_label = format!(
        "{:>width$}",
        format_axis_tokens(max_tokens),
        width = y_label_width as usize
    );
    buf.set_string(inner.x, inner.y, max_label, Style::default().fg(Color::DarkGray));

    let zero_label = format!("{:>width$}", "0", width = y_label_width as usize);
    buf.set_string(
        inner.x,
        inner.y + chart_height as u16 - 1,
        zero_label,
        Style::default().fg(Color::DarkGray),
    );

    for row in 0..chart_height {
        buf.set_string(
            inner.x + y_label_width,
            inner.y + row as u16,
            "│",
            Style::default().fg(Color::DarkGray),
        );
    }

    // 逐日绘制按模型堆叠的彩色柱子
    for (i, day) in days.iter().enumerate() {
        let start_col = (i * chart_width) / bar_count;
        let end_col = ((i + 1) * chart_width) / bar_count;
        if end_col <= start_col {
            continue;
        }
        let bar_cols = if end_col - start_col > 1 {
            end_col - start_col - 1
        } else {
            1
        };

        if day.total_tokens == 0 {
            continue;
        }

        let mut cumulative = 0u64;
        let mut prev_row_top = 0usize;

        for seg in &day.segments {
            cumulative = cumulative.saturating_add(seg.tokens);
            let target_top = ((cumulative as f64 / max_tokens as f64) * chart_height as f64)
                .round() as usize;
            let mut row_top = target_top.min(chart_height);
            if row_top == 0 && cumulative == day.total_tokens {
                row_top = 1;
            }
            if row_top <= prev_row_top {
                continue;
            }

            let style = Style::default().fg(model_color(&seg.model));
            for h in prev_row_top..row_top {
                let y = inner.y + (chart_height - 1 - h) as u16;
                for c in 0..bar_cols {
                    let x = plot_x + (start_col + c) as u16;
                    if x < inner.right() && y < inner.bottom() {
                        buf.set_string(x, y, "█", style);
                    }
                }
            }
            prev_row_top = row_top;
        }
    }

    // 绘制底部横轴线与均匀分布的日期刻度
    let axis_y = inner.y + chart_height as u16;
    buf.set_string(
        inner.x + y_label_width,
        axis_y,
        "└",
        Style::default().fg(Color::DarkGray),
    );
    for c in 0..chart_width {
        buf.set_string(
            plot_x + c as u16,
            axis_y,
            "─",
            Style::default().fg(Color::DarkGray),
        );
    }

    let label_y = axis_y + 1;
    if label_y < inner.bottom() {
        let tick_indices = [0, bar_count / 3, (bar_count * 2) / 3, bar_count - 1];
        let mut last_end = 0u16;
        for &idx in &tick_indices {
            if idx >= bar_count {
                continue;
            }
            let start_col = (idx * chart_width) / bar_count;
            let label = &days[idx].short_label;
            let mut x = plot_x + start_col as u16;
            let label_len = label.len() as u16;
            if x + label_len > inner.right() {
                x = inner.right().saturating_sub(label_len);
            }
            if x >= last_end {
                buf.set_string(x, label_y, label, Style::default().fg(Color::DarkGray));
                last_end = x + label_len + 2;
            }
        }
    }
}
