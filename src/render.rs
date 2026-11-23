//! 单次 CLI 输出的纯文本对齐表格渲染器。
//!
//! 约定：
//! - 输出文案全英文，纯空格对齐，无装饰线条，无 ANSI 颜色（丰富视觉留在 TUI）。
//! - 总计表与所有分组表共用同一套列宽以及全局最大分桶 token 刻度。
//! - 费用统一以美元结算并展示（`$0.123`）。

use crate::aggregate::{BucketRow, ModelGroup, Report};
use crate::pricing::ModelPricing;

/// 7 列的右对齐标志：`[时间桶, Tokens, Chart, Cache%, Cost, TTFT, Speed]`。
const RIGHT_ALIGN: [bool; 7] = [false, true, false, true, true, true, true];

/// 用于计算行内 `Chart` 列宽的默认目标行宽与上下限。
const DEFAULT_TERMINAL_WIDTH: usize = 80;
const MIN_CHART_WIDTH: usize = 8;
const MAX_CHART_WIDTH: usize = 30;

/// 整份报表所有表格共用的列宽与柱状图刻度。
struct Layout {
    widths: [usize; 7],
    chart_width: usize,
    max_tokens: u64,
}

/// 将无符号整数格式化为千分位逗号形式（如 `1,234,567`）。
fn format_int(n: u64) -> String {
    let raw = n.to_string();
    let len = raw.len();
    let mut out = String::with_capacity(len + len / 3);

    for (idx, ch) in raw.chars().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }

    out
}

/// 格式化单行美元费用。
fn format_cost(row: &BucketRow) -> String {
    format!("${:.3}", row.cost_usd)
}

/// 格式化每百万 token 的美元单价：至少保留 2 位小数，低价位保留有效位至最多 4 位。
fn format_unit_price(rate: f64) -> String {
    let fixed = format!("{:.4}", rate);
    let trimmed = fixed.trim_end_matches('0');
    let decimals = match trimmed.find('.') {
        Some(dot) => trimmed.len() - dot - 1,
        None => 0,
    };
    if decimals < 2 {
        format!("{:.2}", rate)
    } else {
        trimmed.to_string()
    }
}

/// 格式化分组标题下的三档参考单价行。
fn format_price_line(pricing: &ModelPricing) -> String {
    format!(
        "price: in miss ${} / hit ${} / out ${} per 1M tok",
        format_unit_price(pricing.input_cache_miss),
        format_unit_price(pricing.input_cache_hit),
        format_unit_price(pricing.output),
    )
}

/// 生成不含 `Chart` 列的 6 个基础单元格文案；空样本用 `—`。
fn base_cells(row: &BucketRow) -> [String; 6] {
    let tokens = format_int(row.total_tokens());

    let cache = match row.cache_percent() {
        Some(pct) => format!("{:.1}%", pct),
        None => "—".to_string(),
    };

    let cost = format_cost(row);

    let ttft = match row.avg_ttft_secs() {
        Some(secs) => format!("{:.1}s", secs),
        None => "—".to_string(),
    };

    let speed = match row.avg_speed_tps() {
        Some(tps) => format!("{}/s", tps.round() as u64),
        None => "—".to_string(),
    };

    [row.label.clone(), tokens, cache, cost, ttft, speed]
}

/// 按字符数计算列宽（避免 `—`、`█` 等多字节 UTF-8 字符导致对齐错位）。
fn char_width(s: &str) -> usize {
    s.chars().count()
}

/// 将单个单元格按左右对齐规则补齐到指定宽度。
fn pad_cell(text: &str, width: usize, right_align: bool) -> String {
    let len = char_width(text);
    if len >= width {
        return text.to_string();
    }
    let pad = " ".repeat(width - len);
    if right_align {
        format!("{}{}", pad, text)
    } else {
        format!("{}{}", text, pad)
    }
}

/// 以双空格连接 7 个对齐后的单元格，并裁掉行尾多余空格。
fn join_row(cells: &[String; 7], widths: &[usize; 7]) -> String {
    let mut parts = Vec::with_capacity(7);
    for i in 0..7 {
        parts.push(pad_cell(&cells[i], widths[i], RIGHT_ALIGN[i]));
    }
    parts.join("  ").trim_end().to_string()
}

/// 扫描总计表与全部分组表，计算统一列宽与全局 `max_tokens`。
fn compute_layout(report: &Report, target_width: usize) -> Layout {
    let first_header = report.granularity.column_header();
    let base_headers = [first_header, "Tokens", "Cache%", "Cost", "TTFT", "Speed"];

    let mut base_widths = [0usize; 6];
    for i in 0..6 {
        base_widths[i] = char_width(base_headers[i]);
    }

    let mut max_tokens: u64 = 0;

    let mut update_from_row = |row: &BucketRow, is_bucket: bool| {
        let cells = base_cells(row);
        for i in 0..6 {
            let w = char_width(&cells[i]);
            if w > base_widths[i] {
                base_widths[i] = w;
            }
        }
        if is_bucket {
            let total = row.total_tokens();
            if total > max_tokens {
                max_tokens = total;
            }
        }
    };

    for row in &report.overall.rows {
        update_from_row(row, true);
    }
    update_from_row(&report.overall.total, false);

    for group in &report.groups {
        for row in &group.rows {
            update_from_row(row, true);
        }
        update_from_row(&group.total, false);
    }

    // 7 列共 6 个双空格间距（12 列），剩余空间分配给 Chart 并夹在 8..=30
    let fixed_width: usize = base_widths.iter().sum::<usize>() + 12;
    let available = target_width.saturating_sub(fixed_width);
    let chart_width = available.clamp(MIN_CHART_WIDTH, MAX_CHART_WIDTH);

    let widths = [
        base_widths[0],
        base_widths[1],
        chart_width,
        base_widths[2],
        base_widths[3],
        base_widths[4],
        base_widths[5],
    ];

    Layout {
        widths,
        chart_width,
        max_tokens,
    }
}

/// 生成单行 token 块状条：所有表共用全局 `max_tokens`，最大值填满整条。
fn token_bar(tokens: u64, layout: &Layout) -> String {
    if layout.max_tokens == 0 {
        return "—".to_string();
    }
    if tokens == 0 {
        return " ".repeat(layout.chart_width);
    }

    let ratio = tokens as f64 / layout.max_tokens as f64;
    let scaled = (ratio * layout.chart_width as f64).round() as usize;
    let filled = scaled.clamp(1, layout.chart_width);
    let empty = layout.chart_width - filled;
    format!("{}{}", "█".repeat(filled), " ".repeat(empty))
}

/// 渲染单张表格（表头 + 各分桶行 + `Total` 行）。
fn render_table(
    first_header: &str,
    rows: &[BucketRow],
    total: &BucketRow,
    layout: &Layout,
) -> Vec<String> {
    let header_cells = [
        first_header.to_string(),
        "Tokens".to_string(),
        "Chart".to_string(),
        "Cache%".to_string(),
        "Cost".to_string(),
        "TTFT".to_string(),
        "Speed".to_string(),
    ];

    let mut lines = Vec::with_capacity(rows.len() + 2);
    lines.push(join_row(&header_cells, &layout.widths));

    for row in rows {
        let base = base_cells(row);
        let chart = token_bar(row.total_tokens(), layout);
        let full = [
            base[0].clone(),
            base[1].clone(),
            chart,
            base[2].clone(),
            base[3].clone(),
            base[4].clone(),
            base[5].clone(),
        ];
        lines.push(join_row(&full, &layout.widths));
    }

    let total_base = base_cells(total);
    let total_full = [
        total_base[0].clone(),
        total_base[1].clone(),
        "—".to_string(),
        total_base[2].clone(),
        total_base[3].clone(),
        total_base[4].clone(),
        total_base[5].clone(),
    ];
    lines.push(join_row(&total_full, &layout.widths));

    lines
}

/// 按默认行宽渲染完整报表。
pub fn render_report(report: &Report) -> String {
    render_report_with_width(report, DEFAULT_TERMINAL_WIDTH)
}

/// 按指定目标行宽渲染完整报表。
pub fn render_report_with_width(report: &Report, width: usize) -> String {
    let mut lines: Vec<String> = Vec::new();

    if report.groups.is_empty() {
        if report.unknown_models.is_empty() {
            return "no usage data in range".to_string();
        }
        return format!("unknown models: {}", report.unknown_models.join(", "));
    }

    let layout = compute_layout(report, width);
    let first_header = report.granularity.column_header();

    lines.push("all models".to_string());
    lines.extend(render_table(
        first_header,
        &report.overall.rows,
        &report.overall.total,
        &layout,
    ));

    for group in &report.groups {
        render_group(&mut lines, first_header, group, &layout);
    }

    if !report.unknown_models.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "unknown models: {}",
            report.unknown_models.join(", ")
        ));
    }

    lines.join("\n")
}

/// 向输出追加一个 `(provider, model)` 分组段落。
fn render_group(lines: &mut Vec<String>, first_header: &str, group: &ModelGroup, layout: &Layout) {
    lines.push(String::new());
    lines.push(format!("{} · {}", group.provider, group.model));
    lines.push(format_price_line(&group.pricing));
    lines.extend(render_table(first_header, &group.rows, &group.total, layout));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_integers_and_unit_prices_cleanly() {
        assert_eq!(format_int(0), "0");
        assert_eq!(format_int(999), "999");
        assert_eq!(format_int(1_234_567), "1,234,567");

        assert_eq!(format_unit_price(2.0), "2.00");
        assert_eq!(format_unit_price(0.04), "0.04");
        assert_eq!(format_unit_price(0.006), "0.006");
        assert_eq!(format_unit_price(0.0028), "0.0028");
    }
}
