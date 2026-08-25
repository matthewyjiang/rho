use pretty_assertions::assert_eq;

use super::{command_identity, short_process_id, ProcessPanel};
use crate::tools::process::{LiveProcessSummary, State};

fn summary(id: &str, command: &str, elapsed_seconds: u64) -> LiveProcessSummary {
    LiveProcessSummary {
        process_id: id.to_owned(),
        command: command.to_owned(),
        state: State::Running,
        elapsed_seconds,
        quiet_seconds: None,
        exit_code: None,
    }
}

// Covers: more than two live processes must not grow the rail past the shared cap.
// Owner: pure layout
#[test]
fn desired_height_caps_at_two_oldest() {
    let mut panel = ProcessPanel::default();
    assert!(panel.replace_processes(vec![
        summary("aaaaaaaa-1111", "sleep 1", 3),
        summary("bbbbbbbb-2222", "sleep 2", 2),
        summary("cccccccc-3333", "sleep 3", 1),
    ]));

    assert_eq!(panel.desired_height(), 2);
    let visible = panel
        .visible_processes(8)
        .iter()
        .map(|process| process.process_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(visible, ["aaaaaaaa-1111", "bbbbbbbb-2222"]);
}

// Covers: whole-second snapshots must not force a redraw every poll.
// Owner: pure layout
#[test]
fn identical_summaries_do_not_mark_the_panel_dirty() {
    let mut panel = ProcessPanel::default();
    let processes = vec![summary("aaaaaaaa-1111", "sleep 60", 4)];
    assert!(panel.replace_processes(processes.clone()));
    assert!(!panel.replace_processes(processes));
}

// Covers: a multiline command must occupy one rail identity field.
// Owner: pure layout
#[test]
fn command_identity_uses_the_first_line_and_short_id() {
    assert_eq!(command_identity("sleep 60\necho still running"), "sleep 60");
    assert_eq!(
        short_process_id("550e8400-e29b-41d4-a716-446655440000"),
        "550e8400"
    );
    assert_eq!(short_process_id("abc"), "abc");
}
