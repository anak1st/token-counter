//! TUI 三段式顶层渲染调度。
//!
//! 布局结构：
//! - 顶部（3 行）：Tabs 导航栏（`Overview` | `Hour` | `Day` | `Week` | `Month` | `Session`）+ 框内 Agent 按钮
//! - 中间（弹性）：当前激活视图内容（首次加载中显示居中动画）
//! - 底部（1 行）：快捷键提示与 60s 自动刷新状态（含可点击未知模型徽章）

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Tabs};
use ratatui::Frame;

use super::style::{spinner_frame, styled_block};
use super::{modals, overview, period, session};
use crate::tui::app::{ActiveModal, App, Tab, UiLayout};

/// 主入口：绘制整帧 TUI 并刷新本帧鼠标交互热区。
pub fn render(frame: &mut Frame, app: &mut App) {
    app.layout = UiLayout::default();

    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, sections[0]);

    if app.snapshot.is_none() {
        render_loading(frame, app, sections[1]);
    } else {
        match app.current_tab {
            Tab::Overview => {
                let hits = if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    Some(overview::render(
                        frame,
                        sections[1],
                        view,
                        snap.is_partial,
                        app.chart_mode,
                        app.chart_offset,
                        app.focus,
                        &mut app.overview_table,
                    ))
                } else {
                    None
                };
                if let Some(hits) = hits {
                    app.layout.lower = Some(hits.lower);
                    app.layout.chart_panel_rect = Some(hits.chart_panel);
                    app.layout.chart_days_rect = hits.chart_days;
                    app.layout.chart_hours_rect = hits.chart_hours;
                    app.layout.chart_prev_rect = hits.chart_prev;
                    app.layout.chart_next_rect = hits.chart_next;
                    app.clamp_overview_window(
                        hits.chart_max_offset,
                        hits.chart_page_step,
                        hits.window_models_len,
                    );
                }
            }
            Tab::Hour => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Hourly",
                            unit_name: "hours",
                            first_col: "Hour",
                        },
                        &view.hourly,
                        &mut app.hour_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Day => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Daily",
                            unit_name: "days",
                            first_col: "Date",
                        },
                        &view.daily,
                        &mut app.day_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Week => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Weekly",
                            unit_name: "weeks",
                            first_col: "Week",
                        },
                        &view.weekly,
                        &mut app.week_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Month => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = period::render(
                        frame,
                        sections[1],
                        period::PeriodViewSpec {
                            panel_name: "Monthly",
                            unit_name: "months",
                            first_col: "Month",
                        },
                        &view.monthly,
                        &mut app.month_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = upper_hit;
                    app.layout.lower = lower_hit;
                }
            }
            Tab::Session => {
                if let Some(snap) = app.snapshot.as_ref() {
                    let view = snap.view(app.agent_filter);
                    let (upper_hit, lower_hit) = session::render(
                        frame,
                        sections[1],
                        &view.sessions,
                        app.agent_filter.is_none(),
                        &mut app.session_table,
                        &mut app.detail_table,
                        app.focus,
                    );
                    app.layout.upper = Some(upper_hit);
                    app.layout.lower = Some(lower_hit);
                }
            }
        }
    }

    render_footer(frame, app, sections[2]);

    // 弹窗打开时覆盖渲染在最顶层
    app.layout.modal = match app.active_modal {
        Some(ActiveModal::AgentPicker) => modals::render_agent_picker(frame, app, area),
        Some(ActiveModal::UnknownModels) => modals::render_unknown_models(frame, app, area),
        None => None,
    };
}

/// 顶部 Tabs 导航栏：边框与其余面板完全一致，右侧框内放置紧凑的 Agent 选择按钮。
fn render_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = styled_block("token-counter");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|&tab| {
            let style = if tab == app.current_tab {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(tab.title(), style))
        })
        .collect();

    let selected = Tab::ALL
        .iter()
        .position(|&t| t == app.current_tab)
        .unwrap_or(0);

    let agent_value_style = if app.agent_filter.is_some() {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let agent_button = Line::from(vec![
        Span::styled("Agent: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.active_agent_label(), agent_value_style),
        Span::styled(" [a] ", Style::default().fg(Color::Cyan)),
    ]);

    let button_width = (agent_button.width() as u16).min(inner.width / 2);
    let inner_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(button_width)])
        .split(inner);

    // 精确记录每个 Tab 与右侧 Agent 按钮的鼠标点击热区
    let mut cursor_x = inner_chunks[0].x;
    let tabs_right = inner_chunks[0].right();
    for (idx, &tab) in Tab::ALL.iter().enumerate() {
        if idx > 0 {
            cursor_x = cursor_x.saturating_add(1);
        }
        let tab_width = (tab.title().len() as u16).saturating_add(2);
        if cursor_x < tabs_right {
            let clamped_w = tab_width.min(tabs_right - cursor_x);
            app.layout
                .tab_rects
                .push((tab, Rect::new(cursor_x, inner_chunks[0].y, clamped_w, 1)));
        }
        cursor_x = cursor_x.saturating_add(tab_width);
    }
    app.layout.agent_button_rect = Some(inner_chunks[1]);

    let tabs = Tabs::new(titles)
        .select(selected)
        .padding(" ", " ")
        .divider(Span::styled("│", Style::default().fg(Color::DarkGray)));

    frame.render_widget(tabs, inner_chunks[0]);
    frame.render_widget(
        Paragraph::new(agent_button).alignment(Alignment::Right),
        inner_chunks[1],
    );
}

/// 首次后台扫描完成前的居中加载态。
fn render_loading(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("Loading");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Length(1),
            Constraint::Percentage(45),
        ])
        .split(inner);

    let line = if let Some(ref err) = app.last_error {
        Line::from(Span::styled(
            format!("Error: {}", err),
            Style::default().fg(Color::Red),
        ))
    } else {
        Line::from(vec![
            Span::styled(
                spinner_frame(app.spinner_tick),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "Scanning local agent sessions...",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };

    let paragraph = Paragraph::new(line).alignment(Alignment::Center);
    frame.render_widget(paragraph, rows[1]);
}

/// 底部提示与 60s 刷新状态栏（同时记录 `N unknown [u]` 可点击热区）。
fn render_footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let hints = match app.active_modal {
        Some(ActiveModal::AgentPicker) => Line::from(vec![
            Span::styled(" ↑/↓/j/k", Style::default().fg(Color::Cyan)),
            Span::styled(" select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::styled(" confirm  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc/a", Style::default().fg(Color::Cyan)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ]),
        Some(ActiveModal::UnknownModels) => Line::from(vec![
            Span::styled(" ↑/↓/j/k", Style::default().fg(Color::Cyan)),
            Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc/u", Style::default().fg(Color::Cyan)),
            Span::styled(" close", Style::default().fg(Color::DarkGray)),
        ]),
        None => {
            let hint_spans = vec![
                Span::styled(" ←/→", Style::default().fg(Color::Cyan)),
                Span::styled(" view  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Tab", Style::default().fg(Color::Cyan)),
                Span::styled(" pane  ", Style::default().fg(Color::DarkGray)),
                Span::styled("a", Style::default().fg(Color::Cyan)),
                Span::styled(" agent  ", Style::default().fg(Color::DarkGray)),
                Span::styled("↑/↓", Style::default().fg(Color::Cyan)),
                Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
                Span::styled("r", Style::default().fg(Color::Cyan)),
                Span::styled(" refresh  ", Style::default().fg(Color::DarkGray)),
                Span::styled("q", Style::default().fg(Color::Cyan)),
                Span::styled(" quit", Style::default().fg(Color::DarkGray)),
            ];
            Line::from(hint_spans)
        }
    };

    let mut unknown_badge_len: u16 = 0;

    let status = if app.refreshing {
        let is_backfilling = app.snapshot.as_ref().is_some_and(|s| s.is_partial);
        let msg = if is_backfilling {
            " 30d ready · loading history... "
        } else {
            " Refreshing today... "
        };
        Line::from(vec![
            Span::styled(
                spinner_frame(app.spinner_tick),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(msg, Style::default().fg(Color::DarkGray)),
        ])
    } else if let Some(ref err) = app.last_error {
        Line::from(Span::styled(
            format!("Error: {} ", err),
            Style::default().fg(Color::Red),
        ))
    } else if let Some(ref snap) = app.snapshot {
        let unknown_count = snap.view(app.agent_filter).unknown_models.len();

        let mut spans = Vec::new();
        if unknown_count > 0 {
            let badge = format!("{} unknown [u]", unknown_count);
            unknown_badge_len = badge.len() as u16;
            spans.push(Span::styled(badge, Style::default().fg(Color::Yellow)));
            spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            format!(
                "Updated {} ({}ms) · next {}s ",
                snap.updated_at,
                snap.load_ms,
                app.secs_until_next_refresh()
            ),
            Style::default().fg(Color::DarkGray),
        ));
        Line::from(spans)
    } else {
        Line::default()
    };

    let status_total_w = status.width() as u16;
    let status_width = status_total_w.min(area.width / 2);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(status_width)])
        .split(area);

    if unknown_badge_len > 0 && chunks[1].width > 0 {
        let start_x = chunks[1].right().saturating_sub(status_total_w.min(chunks[1].width));
        let badge_w = unknown_badge_len.min(chunks[1].right().saturating_sub(start_x));
        if badge_w > 0 {
            app.layout.unknown_badge_rect = Some(Rect::new(start_x, chunks[1].y, badge_w, 1));
        }
    }

    frame.render_widget(Paragraph::new(hints), chunks[0]);
    frame.render_widget(Paragraph::new(status).alignment(Alignment::Right), chunks[1]);
}
