//! 单次 CLI 会话列表与 Turn 明细的纯文本对齐渲染器。

use unicode_width::UnicodeWidthStr;

use crate::core::format::{
    format_compact_tokens, format_cost, format_int, format_percent, format_span, format_speed,
    format_ttft, short_id, truncate_width,
};
use crate::core::time::format_datetime;
use crate::report::session::{SessionDetail, SessionSummary};

/// 渲染纯文本对齐的会话列表（最多展示 `limit` 条）。
pub fn render_session_list(sessions: &[SessionSummary], limit: usize) -> String {
    if sessions.is_empty() {
        return "no sessions found".to_string();
    }

    let slice_len = sessions.len().min(limit);
    let shown = &sessions[..slice_len];

    let headers = [
        "Time", "Agent", "Session", "Name", "Folder", "Turns", "PeakCtx", "Tokens", "Cache%",
        "Cost", "TTFT", "Speed",
    ];
    let right_align = [
        false, false, false, false, false, true, true, true, true, true, true, true,
    ];

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(shown.len());
    for s in shown {
        let name = match s.name.as_deref() {
            Some(text) => truncate_width(text, 30),
            None => "—".to_string(),
        };
        let folder = match s.folder.as_deref() {
            Some(text) => truncate_width(text, 22),
            None => "—".to_string(),
        };
        rows.push(vec![
            format_datetime(s.last_ts_ms),
            s.agent.id().to_string(),
            short_id(&s.session_id, 12),
            name,
            folder,
            s.turns.to_string(),
            format_compact_tokens(s.peak_ctx),
            format_int(s.total_tokens()),
            format_percent(s.cache_percent()),
            format_cost(s.cost_usd),
            format_ttft(s.avg_ttft_secs()),
            format_speed(s.avg_speed_tps()),
        ]);
    }

    render_aligned_table(&headers, &right_align, &rows)
}

/// 渲染单个会话的纯文本总览 + Turn 轮次明细表。
pub fn render_session_detail(detail: &SessionDetail) -> String {
    let s = &detail.summary;
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("agent    {}", s.agent.id()));
    lines.push(format!("session  {}", s.session_id));
    if let Some(ref name) = s.name {
        lines.push(format!("name     {}", name));
    }
    if let Some(ref folder) = s.folder {
        lines.push(format!("folder   {}", folder));
    }
    lines.push(format!(
        "window   {} .. {} ({})",
        format_datetime(s.first_ts_ms),
        format_datetime(s.last_ts_ms),
        format_span(s.span_secs()),
    ));
    lines.push(format!("models   {}", s.models.join(", ")));

    let mut totals = format!(
        "totals   turns {} / steps {} / peak ctx {} / tokens {} / cache {} / cost {} / ttft {} / speed {}",
        s.turns,
        s.steps,
        format_compact_tokens(s.peak_ctx),
        format_int(s.total_tokens()),
        format_percent(s.cache_percent()),
        format_cost(s.cost_usd),
        format_ttft(s.avg_ttft_secs()),
        format_speed(s.avg_speed_tps()),
    );
    if s.aborted_steps > 0 {
        totals.push_str(&format!(" / aborted {}", s.aborted_steps));
    }
    lines.push(totals);
    lines.push(String::new());

    let headers = [
        "Turn", "Time", "Model", "Steps", "PeakCtx", "Tokens", "Cache%", "Cost", "TTFT", "Speed",
        "Span",
    ];
    let right_align = [
        true, false, false, true, true, true, true, true, true, true, true,
    ];

    let mut rows: Vec<Vec<String>> = Vec::with_capacity(detail.turns.len() + 1);
    for t in &detail.turns {
        rows.push(vec![
            format!("#{}", t.index),
            format_datetime(t.first_ts_ms),
            format_models_cell(&t.models),
            t.steps.to_string(),
            format_compact_tokens(t.peak_ctx),
            format_int(t.total_tokens()),
            format_percent(t.cache_percent()),
            format_cost(t.cost_usd),
            format_ttft(t.avg_ttft_secs()),
            format_speed(t.avg_speed_tps()),
            format_span(t.span_secs()),
        ]);
    }

    rows.push(vec![
        "Total".to_string(),
        "—".to_string(),
        "—".to_string(),
        s.steps.to_string(),
        format_compact_tokens(s.peak_ctx),
        format_int(s.total_tokens()),
        format_percent(s.cache_percent()),
        format_cost(s.cost_usd),
        format_ttft(s.avg_ttft_secs()),
        format_speed(s.avg_speed_tps()),
        format_span(s.span_secs()),
    ]);

    lines.push(render_aligned_table(&headers, &right_align, &rows));
    lines.join("\n")
}

/// 渲染通用纯空格对齐表格（无装饰线、无颜色，按终端显示列宽对齐以兼容中文标签）。
fn render_aligned_table(headers: &[&str], right_align: &[bool], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths = vec![0usize; cols];

    for i in 0..cols {
        widths[i] = UnicodeWidthStr::width(headers[i]);
    }
    for row in rows {
        for i in 0..cols {
            let w = UnicodeWidthStr::width(row[i].as_str());
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let mut out = Vec::with_capacity(rows.len() + 1);
    let header_cells: Vec<String> = headers.iter().map(|h| (*h).to_string()).collect();
    out.push(join_cells(&header_cells, &widths, right_align));
    for row in rows {
        out.push(join_cells(row, &widths, right_align));
    }
    out.join("\n")
}

/// 按显示列宽与对齐方向拼接单行。
fn join_cells(cells: &[String], widths: &[usize], right_align: &[bool]) -> String {
    let mut parts = Vec::with_capacity(cells.len());
    for i in 0..cells.len() {
        let len = UnicodeWidthStr::width(cells[i].as_str());
        let pad = widths[i].saturating_sub(len);
        let spaces = " ".repeat(pad);
        if right_align[i] {
            parts.push(format!("{}{}", spaces, cells[i]));
        } else {
            parts.push(format!("{}{}", cells[i], spaces));
        }
    }
    parts.join("  ").trim_end().to_string()
}

/// 格式化 Turn 模型列：无模型显示 `unknown`，多模型按逗号展开。
fn format_models_cell(models: &[String]) -> String {
    if models.is_empty() {
        "unknown".to_string()
    } else {
        truncate_width(&models.join(", "), 36)
    }
}
