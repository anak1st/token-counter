//! TUI 渲染与交互集成测试。

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use token_counter::core::model::Agent;
use token_counter::stats::aggregate::BucketRow;
use token_counter::stats::session::{SessionDetail, SessionSummary, TurnSummary};

use super::render;
use super::style::{format_axis_tokens, nice_axis_ceil};
use crate::tui::app::{ActiveModal, App, ChartMode, FocusPane, KeyAction, Tab};
use crate::tui::data::{ModelRow, PeriodData, TuiSnapshot, ViewSnapshot};

fn empty_view() -> ViewSnapshot {
    ViewSnapshot {
        overall_total: BucketRow::empty("Total"),
        hourly: PeriodData {
            rows: Vec::new(),
            breakdowns: HashMap::new(),
        },
        daily: PeriodData {
            rows: Vec::new(),
            breakdowns: HashMap::new(),
        },
        weekly: PeriodData {
            rows: Vec::new(),
            breakdowns: HashMap::new(),
        },
        monthly: PeriodData {
            rows: Vec::new(),
            breakdowns: HashMap::new(),
        },
        model_rows: Vec::new(),
        sessions: Vec::new(),
        unknown_models: Vec::new(),
    }
}

fn sample_snapshot() -> TuiSnapshot {
    let mut day_row = BucketRow::empty("2026-11-24");
    day_row.requests = 12;
    day_row.input = 500_000;
    day_row.output = 100_000;
    day_row.reasoning = 40_000;
    day_row.cache_read = 4_000_000;
    day_row.cost_usd = 2.5;
    day_row.ttft_sum_ms = 5_000;
    day_row.ttft_count = 2;
    day_row.gen_ms = 1_000;
    day_row.speed_tokens = 120;

    let mut hour_row = day_row.clone();
    hour_row.label = "2026-11-24 10:00".to_string();

    let mut week_row = day_row.clone();
    week_row.label = "2026-W48".to_string();

    let mut month_row = day_row.clone();
    month_row.label = "2026-11".to_string();

    let model_row = ModelRow {
        agent: Agent::Dsh,
        provider: "deepseek-official".to_string(),
        model: "deepseek-v4.1-flash".to_string(),
        metrics: day_row.clone(),
    };

    let make_period = |row: BucketRow| {
        let mut breakdowns = HashMap::new();
        breakdowns.insert(row.label.clone(), vec![model_row.clone()]);
        PeriodData {
            rows: vec![row],
            breakdowns,
        }
    };

    let dsh_session = SessionDetail {
        summary: SessionSummary {
            agent: Agent::Dsh,
            session_id: "019ab3c4-demo-session".to_string(),
            name: Some("Demo Session Title".to_string()),
            folder: Some("token-counter".to_string()),
            first_ts_ms: 1_763_949_600_000,
            last_ts_ms: 1_763_950_200_000,
            models: vec!["deepseek-v4.1-flash".to_string()],
            turns: 1,
            steps: 3,
            aborted_steps: 0,
            peak_ctx: 162_000,
            input: 100_000,
            output: 20_000,
            reasoning: 0,
            cache_read: 500_000,
            cache_write: 0,
            cost_usd: 0.42,
            ttft_sum_ms: 3_000,
            ttft_count: 2,
            gen_ms: 1_000,
            speed_tokens: 110,
        },
        turns: vec![TurnSummary {
            index: 1,
            turn_id: "1".to_string(),
            first_ts_ms: 1_763_949_600_000,
            last_ts_ms: 1_763_950_200_000,
            models: vec!["deepseek-v4.1-flash".to_string()],
            steps: 3,
            aborted_steps: 0,
            peak_ctx: 162_000,
            input: 100_000,
            output: 20_000,
            reasoning: 0,
            cache_read: 500_000,
            cache_write: 0,
            cost_usd: 0.42,
            ttft_sum_ms: 3_000,
            ttft_count: 2,
            gen_ms: 1_000,
            speed_tokens: 110,
        }],
    };

    let mut older_day = day_row.clone();
    older_day.label = "2026-09-01".to_string();

    let mut daily_breakdowns = HashMap::new();
    daily_breakdowns.insert(day_row.label.clone(), vec![model_row.clone()]);
    daily_breakdowns.insert(older_day.label.clone(), vec![model_row.clone()]);
    let daily_period = PeriodData {
        rows: vec![day_row.clone(), older_day],
        breakdowns: daily_breakdowns,
    };

    let dsh_view = ViewSnapshot {
        overall_total: day_row.clone(),
        hourly: make_period(hour_row),
        daily: daily_period,
        weekly: make_period(week_row),
        monthly: make_period(month_row),
        model_rows: vec![model_row],
        sessions: vec![dsh_session],
        unknown_models: vec!["opencode · openrouter · test-new-model".to_string()],
    };

    let mut by_agent: HashMap<Agent, ViewSnapshot> = HashMap::new();
    by_agent.insert(Agent::Opencode, empty_view());
    by_agent.insert(Agent::Dsh, dsh_view.clone());
    by_agent.insert(Agent::Agy, empty_view());

    TuiSnapshot {
        all: dsh_view,
        by_agent,
        is_partial: false,
        updated_at: "09:38:00".to_string(),
        load_ms: 42,
    }
}

#[test]
fn renders_loading_all_tabs_and_agent_picker_modal() {
    let backend = TestBackend::new(136, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = App::new();

    // 首次加载态
    terminal.draw(|f| render(f, &mut app)).unwrap();

    // 填充快照后依次渲染全部 6 个 Tab（默认 All 模式，含 Session）
    app.apply_snapshot(sample_snapshot());
    for tab in Tab::ALL {
        app.current_tab = tab;
        terminal.draw(|f| render(f, &mut app)).unwrap();
    }

    // 在 `Overview` 验证点击 `[< Prev]` / `[Next >]` 移动时间窗、点击顶栏 `Hours` 切换以及底栏 `1 unknown [u]`
    app.switch_tab(Tab::Overview);
    terminal.draw(|f| render(f, &mut app)).unwrap();
    assert!(app.layout.chart_max_offset > 0);
    let prev_rect = app.layout.chart_prev_rect.unwrap();
    let next_rect = app.layout.chart_next_rect.unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: prev_rect.x + 1,
        row: prev_rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.chart_offset > 0);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: next_rect.x + 1,
        row: next_rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.chart_offset, 0);

    let hours_rect = app.layout.chart_hours_rect.unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: hours_rect.x + 1,
        row: hours_rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.chart_mode, ChartMode::Hours);
    assert_eq!(app.chart_offset, 0);

    let badge_rect = app.layout.unknown_badge_rect.unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: badge_rect.x + 1,
        row: badge_rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.active_modal, Some(ActiveModal::UnknownModels));
    terminal.draw(|f| render(f, &mut app)).unwrap();
    let unknown_buf: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(unknown_buf.contains("Unknown Models (1)"));
    assert!(unknown_buf.contains("opencode · openrouter · test-new-model"));

    // 点击弹窗外侧关闭
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.active_modal, None);

    // 在 `Day` Tab 下通过 `Tab` 切到下方 Models 子框，再通过 Esc 退回上表
    app.switch_tab(Tab::Day);
    assert_eq!(app.focus, FocusPane::Upper);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.focus, FocusPane::Lower);
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        KeyAction::None
    );
    assert_eq!(app.focus, FocusPane::Upper);

    // 鼠标点击顶部 `Session` Tab 切换视图
    terminal.draw(|f| render(f, &mut app)).unwrap();
    let session_tab_rect = app
        .layout
        .tab_rects
        .iter()
        .find(|(t, _)| *t == Tab::Session)
        .unwrap()
        .1;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: session_tab_rect.x + 1,
        row: session_tab_rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.current_tab, Tab::Session);

    // `Session` Tab 默认处于 All 模式：标题为 `Sessions (1)`，行首带 agent 前缀
    terminal.draw(|f| render(f, &mut app)).unwrap();
    let all_session_buf: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(all_session_buf.contains("Sessions (1)"));
    assert!(all_session_buf.contains("Agent: all [a]"));
    assert!(all_session_buf.contains("dsh · 019ab3c4"));
    assert!(all_session_buf.contains("Demo Session Title"));
    assert!(all_session_buf.contains("token-counter"));
    assert!(all_session_buf.contains("Turns · dsh · 019ab3c4-dem · Demo Session Title (1 turns)"));

    // 鼠标点击顶部 `Agent: all [a]` 按钮打开弹窗，并点击弹窗中的 `dsh` 行确认
    let btn_rect = app.layout.agent_button_rect.unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: btn_rect.x + 1,
        row: btn_rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.active_modal, Some(ActiveModal::AgentPicker));
    terminal.draw(|f| render(f, &mut app)).unwrap();
    let modal_buf: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(modal_buf.contains("Select Agent"));
    assert!(modal_buf.contains("all · All Agents"));
    assert!(modal_buf.contains("dsh · DeepSeek Harness"));

    let modal_rows = app.layout.modal.unwrap().rows.unwrap();
    // 第 2 行索引（all=0, opencode=1, dsh=2）
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: modal_rows.x + 2,
        row: modal_rows.y + 2,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.active_modal, None);
    assert_eq!(app.agent_filter, Some(Agent::Dsh));

    // 鼠标点击下方 Turns 面板：焦点自动切到 Lower
    terminal.draw(|f| render(f, &mut app)).unwrap();
    let lower_rows = app.layout.lower.unwrap().rows.unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: lower_rows.x + 2,
        row: lower_rows.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.focus, FocusPane::Lower);

    terminal.draw(|f| render(f, &mut app)).unwrap();
    let content: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(content.contains("Agent: dsh [a]"));
    assert!(content.contains("Sessions (1)"));
    assert!(content.contains("019ab3c4-dem"));
    assert!(content.contains("Demo Session Title"));
    assert!(content.contains("token-counter"));
    assert!(content.contains("Turns · 019ab3c4-dem · Demo Session Title (1 turns)"));
    assert!(content.contains("#1"));

    // Esc 先从下表退回上表，不直接退出
    assert_eq!(
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        KeyAction::None
    );
    assert_eq!(app.focus, FocusPane::Upper);
}

#[test]
fn rounds_axis_ceiling_to_single_significant_digit() {
    assert_eq!(format_axis_tokens(nice_axis_ceil(0)), "0");
    assert_eq!(format_axis_tokens(nice_axis_ceil(750)), "800");
    assert_eq!(format_axis_tokens(nice_axis_ceil(12_500)), "20k");
    assert_eq!(format_axis_tokens(nice_axis_ceil(320_000)), "400k");
    assert_eq!(format_axis_tokens(nice_axis_ceil(950_000)), "1M");
    assert_eq!(format_axis_tokens(nice_axis_ceil(42_300_000)), "50M");
    assert_eq!(format_axis_tokens(nice_axis_ceil(195_000_000)), "200M");
    assert_eq!(format_axis_tokens(nice_axis_ceil(1_200_000_000)), "2B");
}
