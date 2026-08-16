use ratatui::text::Line;

use super::super::{commands, goal, tests::test_app, ChatMedia, ChatTextDocument, TurnPrompt};
use super::{HeldTurn, HeldTurnWait};

fn attached_document() -> ChatMedia {
    ChatMedia::TextDocument(ChatTextDocument {
        name: "queued.txt".into(),
        mime: "text/plain".into(),
        body: "private attachment".into(),
        truncated: false,
        warnings: Vec::new(),
    })
}

fn assert_goal_command_takes_media(command: &str) {
    let mut app = test_app();
    app.input_ui
        .push_ready_attachment(attached_document(), None);
    app.input_ui.with_text_mut(|text| text.push_str(command));
    let invocation = commands::parse_command(command).unwrap().unwrap();

    let submission = app.take_command_submission(invocation, command.to_owned());

    assert_eq!(submission.media_len(), 1);
    assert!(app.input_ui.attachments().is_empty());
    assert!(app.input_ui.text().is_empty());
}

// Covers: status and early-return goal commands must not leave attachments in composer state.
// Owner: slash-command submission ownership
#[test]
fn goal_status_takes_queued_media() {
    assert_goal_command_takes_media("/goal");
}

#[test]
fn goal_clear_takes_queued_media() {
    assert_goal_command_takes_media("/goal clear");
}

#[test]
fn goal_resume_takes_queued_media() {
    assert_goal_command_takes_media("/goal resume");
}

#[test]
fn invalid_overlong_goal_takes_queued_media() {
    let condition = "x".repeat(goal::MAX_GOAL_CHARS + 1);
    assert_goal_command_takes_media(&format!("/goal {condition}"));
}

fn held_turn(display: &str, wait: HeldTurnWait) -> HeldTurn {
    HeldTurn {
        turn: TurnPrompt::standard(display.to_owned(), display.to_owned()),
        media: Vec::new(),
        paste_segments: Vec::new(),
        wait,
    }
}

// Covers: esc must hand held turns back to the composer, newest first, and
// must not overwrite text typed since the hold.
// Owner: idle input key handling
#[test]
fn esc_takes_back_held_turns_newest_first() {
    let mut app = test_app();
    app.held_turns
        .push_back(held_turn("first", HeldTurnWait::McpConnect));
    app.held_turns
        .push_back(held_turn("second", HeldTurnWait::McpConnect));

    app.take_back_held_turn();
    assert_eq!(app.input_ui.text(), "second");
    assert_eq!(app.held_turns.len(), 1);

    // The composer now holds the recovered prompt, so a second esc must leave
    // it alone rather than replace it with the older hold.
    app.take_back_held_turn();
    assert_eq!(app.input_ui.text(), "second");
    assert_eq!(app.held_turns.len(), 1);
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

// Covers: compact-time Enter/Alt+Enter use the pending-input list, not held turns.
// Owner: idle input key handling
#[test]
fn compact_prompts_use_the_pending_input_list() {
    let mut app = test_app();
    app.begin_compact_ui();
    app.queue_steering_prompt("steer me".into(), "steer me".into(), Vec::new())
        .unwrap();
    app.queue_prompt(
        "next turn".into(),
        "next turn".into(),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();

    assert!(app.held_turns.is_empty());
    assert_eq!(app.pending.steering_prompts().len(), 1);
    assert_eq!(app.pending.queued_prompts().len(), 1);
    let lines = app.pending_input_lines(80);
    assert!(line_text(&lines[0]).contains("1 steer"));
    assert!(line_text(&lines[0]).contains("1 follow-up"));
    assert!(line_text(&lines[1]).contains("STEER"));
    assert!(line_text(&lines[2]).contains("NEXT"));
    assert!(line_text(&lines[2]).contains("after compact"));
}

// Covers: an MCP hold must not start a turn while a compact job holds the session.
// Owner: idle input hold/release
#[test]
fn mcp_holds_are_not_releasable_while_compacting() {
    let mut app = test_app();
    app.held_turns
        .push_back(held_turn("after mcp", HeldTurnWait::McpConnect));

    assert_eq!(
        app.first_releasable_held_wait(false, false),
        Some(HeldTurnWait::McpConnect)
    );
    assert_eq!(app.first_releasable_held_wait(false, true), None);
    assert_eq!(app.first_releasable_held_wait(true, false), None);
}
