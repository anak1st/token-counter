//! 单次 CLI 输出的纯文本对齐表格渲染器。
//!
//! 约定：
//! - 输出文案全英文，纯空格对齐，无装饰线条，无 ANSI 颜色，无图表（丰富视觉全部留给 TUI）。
//! - 总计表与所有分组表共用同一套 6 列宽度，保证整段输出纵向对齐。
//! - 费用统一以美元结算并展示（`$0.123`）。

use crate::aggregate::{BucketRow, ModelGroup, Report};
use crate::pricing::ModelPricing;

/// 6 列的右对齐标志：`[时间桶, Tokens, Cache%, Cost, TTFT, Speed]`。
const RIGHT_ALIGN: [bool; 6] = [false, true, true, true, true, true];

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

/// 生成单行的 6 个单元格文案；空样本用 `—`。
fn row_cells(row: &BucketRow) -> [String; 6] {
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

/// 按字符数计算列宽（避免 `—` 等多字节 UTF-8 字符导致对齐错位）。
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

/// 以双空格连接 6 个对齐后的单元格，并裁掉行尾多余空格。
fn join_row(cells: &[String; 6], widths: &[usize; 6]) -> String {
    let mut parts = Vec::with_capacity(6);
    for i in 0..6 {
        parts.push(pad_cell(&cells[i], widths[i], RIGHT_ALIGN[i]));
    }
    parts.join("  ").trim_end().to_string()
}

/// 扫描总计表与全部分组表，计算全局统一的 6 列列宽。
fn compute_widths(report: &Report) -> [usize; 6] {
    let first_header = report.granularity.column_header();
    let headers = [first_header, "Tokens", "Cache%", "Cost", "TTFT", "Speed"];

    let mut widths = [0usize; 6];
    for i in 0..6 {
        widths[i] = char_width(headers[i]);
    }

    let mut update_from_row = |row: &BucketRow| {
        let cells = row_cells(row);
        for i in 0..6 {
            let w = char_width(&cells[i]);
            if w > widths[i] {
                widths[i] = w;
            }
        }
    };

    for row in &report.overall.rows {
        update_from_row(row);
    }
    update_from_row(&report.overall.total);

    for group in &report.groups {
        for row in &group.rows {
            update_from_row(row);
        }
        update_from_row(&group.total);
    }

    widths
}

/// 渲染单张表格（表头 + 各分桶行 + `Total` 行）。
fn render_table(
    first_header: &str,
    rows: &[BucketRow],
    total: &BucketRow,
    widths: &[usize; 6],
) -> Vec<String> {
    let header_cells = [
        first_header.to_string(),
        "Tokens".to_string(),
        "Cache%".to_string(),
        "Cost".to_string(),
        "TTFT".to_string(),
        "Speed".to_string(),
    ];

    let mut lines = Vec::with_capacity(rows.len() + 2);
    lines.push(join_row(&header_cells, widths));

    for row in rows {
        lines.push(join_row(&row_cells(row), widths));
    }

    lines.push(join_row(&row_cells(total), widths));
    lines
}

/// 渲染完整纯文本对齐报表。
pub fn render_report(report: &Report) -> String {
    let mut lines: Vec<String> = Vec::new();

    if report.groups.is_empty() {
        if report.unknown_models.is_empty() {
            return "no usage data in range".to_string();
        }
        return format!("unknown models: {}", report.unknown_models.join(", "));
    }

    let widths = compute_widths(report);
    let first_header = report.granularity.column_header();

    lines.push("all models".to_string());
    lines.extend(render_table(
        first_header,
        &report.overall.rows,
        &report.overall.total,
        &widths,
    ));

    for group in &report.groups {
        render_group(&mut lines, first_header, group, &widths);
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
fn render_group(lines: &mut Vec<String>, first_header: &str, group: &ModelGroup, widths: &[usize; 6]) {
    lines.push(String::new());
    lines.push(format!("{} · {}", group.provider, group.model));
    lines.push(format_price_line(&group.pricing));
    lines.extend(render_table(first_header, &group.rows, &group.total, widths));
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
