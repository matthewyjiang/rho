use super::*;

fn agent(
    id: &str,
    agent_id: &str,
    state: RunState,
    activity: Option<&str>,
    elapsed_seconds: u64,
) -> RunningSubagent {
    RunningSubagent {
        id: id.into(),
        agent_id: agent_id.into(),
        state,
        last_activity: activity.map(str::to_string),
        elapsed_seconds,
    }
}

fn text(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

#[test]
fn renders_running_agents_with_identity_activity_and_elapsed_time() {
    let panel = SubagentPanel {
        agents: vec![
            agent(
                "a1b2c3",
                "explorer",
                RunState::Running,
                Some("tool: read_file"),
                42,
            ),
            agent(
                "d4e5f6",
                "reviewer",
                RunState::Running,
                Some("assistant text"),
                75,
            ),
        ],
    };

    assert_eq!(
        text(&panel.lines(80, 3)),
        vec![
            "  ├ explorer  a1b2c3  ·  read_file                   42s",
            "  └ reviewer  d4e5f6  ·  responding               1m 15s",
        ]
    );
}

#[test]
fn summarizes_overflow_and_truncates_details_to_width() {
    let panel = SubagentPanel {
        agents: vec![
            agent(
                "a1b2c3",
                "explorer",
                RunState::Running,
                Some("reading a very long filename"),
                1,
            ),
            agent("d4e5f6", "reviewer", RunState::Running, None, 2),
            agent("012abc", "worker", RunState::Running, None, 3),
        ],
    };

    let lines = text(&panel.lines(32, 3));

    assert_eq!(panel.count(), 3);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("  ├ explorer"));
    assert!(
        crate::tui::render::display_width(&lines[1]) <= 32,
        "{}",
        lines[1]
    );
    assert!(!lines.iter().any(|line| line.contains("worker")));
}

#[test]
fn active_tui_frame_places_panel_above_the_composer() {
    let mut app = crate::tui::tests::test_app();
    app.subagent_panel = SubagentPanel {
        agents: vec![agent(
            "a1b2c3",
            "explorer",
            RunState::Running,
            Some("tool: read_file"),
            42,
        )],
    };

    let layout = app.screen_layout(
        ratatui::layout::Rect::new(0, 0, 60, 12),
        std::time::Instant::now(),
    );
    let lines = text(&app.active_lines_for_height(60, 12));
    let panel = lines
        .iter()
        .position(|line| line.contains("1 agent working"))
        .unwrap();

    assert_eq!(layout.subagents.height, 1);
    assert!(layout.activity.is_some());
    assert!(layout.subagents.bottom() <= layout.composer.y);
    assert!(lines[panel + 1].contains("explorer  a1b2c3"));
}

#[test]
fn text_selection_uses_rendered_history_window_with_active_subagents() {
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};

    let mut app = crate::tui::tests::test_app();
    app.running = true;
    app.subagent_panel = SubagentPanel {
        agents: vec![
            agent("a1b2c3", "explorer", RunState::Running, None, 3),
            agent("d4e5f6", "reviewer", RunState::Running, None, 4),
        ],
    };
    for index in 0..20 {
        app.record_inserted_entry(crate::tui::Entry::User(format!("message {index}")));
    }
    let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();

    let now = std::time::Instant::now();
    let layout = app.screen_layout(Rect::new(0, 0, 60, 16), now);
    let (history_start, history_count) =
        app.visible_history_window(layout.history_len, layout.history.height as usize);
    assert_eq!(history_count + 1, layout.history.height as usize);
    let lines = app.history_lines(60, now);
    let target_line = (history_start..history_start + history_count)
        .find(|&line| lines[line].to_string().contains("message"))
        .unwrap();
    let row = layout.history.y + (target_line - history_start) as u16;

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        2,
        row,
        &mut terminal,
    )
    .unwrap();

    assert_eq!(
        app.text_selection.unwrap().selected_line_range(),
        target_line..target_line + 1
    );
}

#[test]
fn activity_rail_shares_a_row_with_jump_to_bottom() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = crate::tui::tests::test_app();
    app.running = true;
    app.subagent_panel = SubagentPanel {
        agents: vec![
            agent("a1b2c3", "explorer", RunState::Running, None, 3),
            agent("d4e5f6", "reviewer", RunState::Running, None, 4),
        ],
    };
    for index in 0..20 {
        app.push_transcript_entry(crate::tui::Entry::User(format!("message {index}")));
    }
    app.scroll_history_page_up(80, 12, std::time::Instant::now());
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let layout = app.screen_layout(
        ratatui::layout::Rect::new(0, 0, 80, 12),
        std::time::Instant::now(),
    );
    let activity = layout.activity.unwrap();
    let jump = layout.jump_to_bottom.unwrap();
    let row = (0..80)
        .map(|x| terminal.backend().buffer()[(x, activity.y)].symbol())
        .collect::<String>();
    assert_eq!(activity.y, jump.y);
    assert!(activity.right() < jump.x);
    assert!(row.contains("working  ·  2 agents"), "{row:?}");
    assert!(row.contains("jump to bottom"), "{row:?}");

    let buffer = terminal.backend().buffer();
    let activity_background = Theme::activity_rail().bg.unwrap();
    for y in activity.y..layout.subagents.bottom() {
        for x in 0..80 {
            assert_eq!(buffer[(x, y)].bg, activity_background);
        }
    }
    for y in layout.top_divider.y..layout.statusline.bottom() {
        for x in 0..80 {
            assert_ne!(buffer[(x, y)].bg, activity_background);
        }
    }
}

#[test]
fn renders_one_agent_detail_when_only_one_row_is_available() {
    let panel = SubagentPanel {
        agents: vec![agent("a1b2c3", "worker", RunState::Running, None, 3)],
    };

    assert!(text(&panel.lines(20, 1))[0].starts_with("  └ worker"));
    assert_eq!(panel.desired_height(), 1);
}
