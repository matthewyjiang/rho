use super::super::{
    commands, goal, tests::test_app, ChatMedia, ChatTextDocument, PasteSegment, TurnPrompt,
};
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

// Covers: compact-held turns keep paste segments so esc restores the typed prompt.
// Owner: idle input key handling
#[test]
fn esc_restores_compact_held_paste_segments() {
    let mut app = test_app();
    app.held_turns.push_back(HeldTurn {
        turn: TurnPrompt::standard("secret".into(), "aaaa".into()),
        media: Vec::new(),
        paste_segments: vec![PasteSegment {
            start: 0,
            marker_len: 4,
            content: "secret".into(),
        }],
        wait: HeldTurnWait::Compact,
    });

    app.take_back_held_turn();
    assert_eq!(app.input_ui.text(), "aaaa");
    assert_eq!(app.input_ui.paste_segments().len(), 1);
}

// Covers: cancelling compact must not auto-start held turns; only a finished
// compact promotes them to Ready.
// Owner: idle input hold/release
#[test]
fn compact_holds_are_not_releasable_until_promoted() {
    let mut app = test_app();
    app.held_turns
        .push_back(held_turn("during compact", HeldTurnWait::Compact));

    assert_eq!(app.first_releasable_held_wait(false), None);

    app.promote_compact_holds();
    assert_eq!(
        app.first_releasable_held_wait(false),
        Some(HeldTurnWait::Ready)
    );
}
