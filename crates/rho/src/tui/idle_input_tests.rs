use super::super::{commands, goal, tests::test_app, ChatMedia, ChatTextDocument};

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
