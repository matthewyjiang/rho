use super::*;

fn agent(
    id: &str,
    preset: &str,
    state: RunState,
    activity: Option<&str>,
    elapsed_seconds: u64,
) -> RunningSubagent {
    RunningSubagent {
        id: id.into(),
        preset: preset.into(),
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
            "● 2 subagents running",
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

    assert_eq!(lines[0], "● 3 subagents running  +1 more");
    assert_eq!(lines.len(), 3);
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
        .position(|line| line.contains("1 subagent running"))
        .unwrap();

    assert_eq!(layout.subagents.height, 2);
    assert!(layout.subagents.bottom() <= layout.composer.y);
    assert!(lines[panel + 1].contains("explorer  a1b2c3"));
}

#[test]
fn collapses_to_header_when_only_one_row_is_available() {
    let panel = SubagentPanel {
        agents: vec![agent("a1b2c3", "worker", RunState::Running, None, 3)],
    };

    assert_eq!(text(&panel.lines(20, 1)), vec!["● 1 subagent running"]);
    assert_eq!(panel.desired_height(), 2);
}
