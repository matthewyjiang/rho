use super::super::{commands, goal, tests::test_app, ChatMedia, ChatTextDocument, TurnPrompt};
use super::PendingMcpSubmission;

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

fn held_turn(display: &str) -> PendingMcpSubmission {
    PendingMcpSubmission {
        turn: TurnPrompt::standard(display.to_owned(), display.to_owned()),
        media: Vec::new(),
        paste_segments: Vec::new(),
    }
}

// Covers: esc must hand turns held during MCP connect back to the composer,
// newest first, and must not overwrite text typed since the hold.
// Owner: idle input key handling
#[test]
fn esc_takes_back_held_turns_newest_first() {
    let mut app = test_app();
    app.pending_mcp_submissions.push_back(held_turn("first"));
    app.pending_mcp_submissions.push_back(held_turn("second"));

    app.take_back_held_turn();
    assert_eq!(app.input_ui.text(), "second");
    assert_eq!(app.pending_mcp_submissions.len(), 1);

    // The composer now holds the recovered prompt, so a second esc must leave
    // it alone rather than replace it with the older hold.
    app.take_back_held_turn();
    assert_eq!(app.input_ui.text(), "second");
    assert_eq!(app.pending_mcp_submissions.len(), 1);
}
