//! CLI 与 TUI 共用的指标与文本格式化辅助函数。
//!
//! 集中维护数值、费用、速度、耗时与宽字符截断逻辑，确保单次纯文本报表与交互式 TUI
//! 的展示口径严格一致且不重复实现。

use std::path::Path;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 千分位逗号整数格式化（如 `138,557,002`）。
pub fn format_int(n: u64) -> String {
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

/// 统一按百万单位 `M` 格式化 token（保留 3 位小数，精确到 1k），
/// 确保 TUI 中 `In / Out / Cache / Tokens` 全部同量纲且小数点垂直对齐。
pub fn format_tokens_m(n: u64) -> String {
    if n == 0 {
        return "—".to_string();
    }
    if n < 500 {
        return "<0.001M".to_string();
    }
    format!("{:.3}M", n as f64 / 1_000_000.0)
}

/// 格式化紧凑上下文大小（如 `162k`、`1.2M`），用于 `PeakCtx` 列。
pub fn format_compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", (tokens as f64 / 1_000.0).round() as u64)
    } else if tokens == 0 {
        "—".to_string()
    } else {
        tokens.to_string()
    }
}

/// 格式化美元费用（保留 3 位小数，如 `$1.234`）。
pub fn format_cost(usd: f64) -> String {
    format!("${:.3}", usd)
}

/// 格式化百分比（保留 1 位小数，空值显示 `—`）。
pub fn format_percent(val: Option<f64>) -> String {
    match val {
        Some(v) => format!("{:.1}%", v),
        None => "—".to_string(),
    }
}

/// 格式化平均首 token 延迟（秒，保留 1 位小数，空值显示 `—`）。
pub fn format_ttft(secs: Option<f64>) -> String {
    match secs {
        Some(v) => format!("{:.1}s", v),
        None => "—".to_string(),
    }
}

/// 格式化平均纯解码速度（`tok/s` 整数，空值显示 `—`）。
pub fn format_speed(tps: Option<f64>) -> String {
    match tps {
        Some(v) => format!("{}/s", v.round() as u64),
        None => "—".to_string(),
    }
}

/// 格式化时间跨度（如 `42s`、`14m`、`2h15m`）。
pub fn format_span(secs: u64) -> String {
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    let rem_mins = mins % 60;
    if rem_mins == 0 {
        format!("{}h", hours)
    } else {
        format!("{}h{}m", hours, rem_mins)
    }
}

/// 截取会话 ID 前 `max_chars` 个字符。
pub fn short_id(id: &str, max_chars: usize) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= max_chars {
        id.to_string()
    } else {
        chars[..max_chars].iter().collect()
    }
}

/// 去除首尾空白；若为空白串则返回 `None`。
pub fn non_empty_trimmed(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 从工作目录路径提取末级目录名（如 `/Users/me/Code/app` -> `app`）。
pub fn folder_name_from_path(raw: &str) -> Option<String> {
    let name = Path::new(raw.trim()).file_name()?.to_str()?;
    non_empty_trimmed(name)
}

/// 按终端显示宽度截断过长文本并补省略号，防止中英文混排撑破列宽。
pub fn truncate_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_durations_and_cjk_truncation() {
        assert_eq!(format_int(0), "0");
        assert_eq!(format_int(999), "999");
        assert_eq!(format_int(1_234_567), "1,234,567");

        assert_eq!(format_tokens_m(0), "—");
        assert_eq!(format_tokens_m(200), "<0.001M");
        assert_eq!(format_tokens_m(1_250_000), "1.250M");

        assert_eq!(format_compact_tokens(0), "—");
        assert_eq!(format_compact_tokens(162_400), "162k");
        assert_eq!(format_compact_tokens(1_250_000), "1.2M");

        assert_eq!(format_span(42), "42s");
        assert_eq!(format_span(840), "14m");
        assert_eq!(format_span(8100), "2h15m");

        assert_eq!(truncate_width("hello", 10), "hello");
        assert_eq!(truncate_width("中文标题测试", 7), "中文标…");
    }
}
