//! TUI 公共样式、布局几何、滚动条与坐标轴刻度辅助。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{
    Block, BorderType, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

/// 加载动画帧序列。
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 模型配色表（兼容绝大多数深浅终端的 ANSI 命名色）。
const MODEL_PALETTE: [Color; 7] = [
    Color::Cyan,
    Color::Magenta,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::LightRed,
    Color::LightCyan,
];

/// 返回当前加载动画帧字符。
pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

/// 统一将主区域按 60% / 40% 拆分为「上半主视图 + 下半联动明细面板」，
/// 确保在各个 Tab 之间切换时底部框高度完全对齐不跳动。
pub fn split_upper_and_models(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    (chunks[0], chunks[1])
}

/// 创建统一圆角边框面板（透明底色，自然融入用户终端主题）。
pub fn styled_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
}

/// 创建带聚焦状态切换的圆角边框面板（聚焦时纯白亮框，失焦时深灰边框）。
pub fn panel_block(title: &str, focused: bool) -> Block<'_> {
    if focused {
        styled_block(title).border_style(Style::default().fg(Color::White))
    } else {
        styled_block(title)
    }
}

/// 从带 1 行表头的表格内区算出实际数据行区域（供鼠标行点击命中计算）。
pub fn table_rows_rect(inner: Rect) -> Rect {
    Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    }
}

/// 在给定区域内计算居中矩形（供各弹窗共用）。
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

/// 统一的表格选中行高亮样式（当前聚焦框）。
pub fn highlight_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

/// 上半主表行高亮样式：聚焦时青色高亮，失焦时暗灰反白以保留当前选中分桶/会话指示。
pub fn upper_highlight_style(focused: bool) -> Style {
    if focused {
        highlight_style()
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::REVERSED)
    }
}

/// 统一的表格表头样式。
pub fn table_header_style() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// 当表格数据行数超过可视区域时，在右边框绘制垂直滚动条。
pub fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    total_rows: usize,
    selected: Option<usize>,
) {
    let visible_rows = inner.height.saturating_sub(1) as usize;
    if total_rows > visible_rows {
        let mut sb_state = ScrollbarState::new(total_rows).position(selected.unwrap_or(0));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb_state,
        );
    }
}

/// 为模型分配确定性颜色（同一模型在柱状图、Overview 与各明细表中保持同色）。
pub fn model_color(model: &str) -> Color {
    if model.contains("deepseek") {
        return Color::Cyan;
    }
    if model.contains("neon") {
        return Color::Magenta;
    }
    if model.contains("hydrogen") {
        return Color::Green;
    }
    if model.contains("glm") {
        return Color::Yellow;
    }
    if model.contains("argon") || model.contains("carbon") {
        return Color::Blue;
    }
    let mut hash: usize = 0;
    for b in model.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(usize::from(b));
    }
    MODEL_PALETTE[hash % MODEL_PALETTE.len()]
}

/// 将柱状图最大值向上取整为 1 位有效数字的整数（如 `195M → 200M`、`42M → 50M`、`320k → 400k`）。
pub fn nice_axis_ceil(raw: u64) -> u64 {
    if raw == 0 {
        return 0;
    }
    let mut pow10 = 1u64;
    while pow10 <= raw / 10 {
        pow10 *= 10;
    }
    let lead = raw.div_ceil(pow10);
    lead.saturating_mul(pow10)
}

/// 紧凑整数格式化（用于柱状图 Y 轴 1 位有效数字整刻度，如 `200M`、`50M`、`400k`、`1B`）。
pub fn format_axis_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{}B", n / 1_000_000_000)
    } else if n >= 1_000_000 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}
