use ratatui::text::Line;

use super::super::{
    activity::ActivityStatus, app_state::SessionUiPhase, command_palette::slash_command_args,
    commands, goal, paste_burst, tests::test_app, ActivityPhase, ChatMedia, ChatTextDocument,
    TurnPrompt,
};
use super::HeldTurn;
use crate::tui::send_confirm::SendSubmission;

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

    let submission = app.take_command_submission(
        invocation,
        TurnPrompt::standard(command.to_owned(), command.to_owned()),
    );

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

fn held_turn(display: &str) -> HeldTurn {
    HeldTurn {
        turn: TurnPrompt::standard(display.to_owned(), display.to_owned()),
        media: Vec::new(),
        paste_segments: Vec::new(),
    }
}

// Covers: esc must hand held turns back to the composer, newest first, and
// must not overwrite text typed since the hold.
// Owner: idle input key handling
#[test]
fn esc_takes_back_held_turns_newest_first() {
    let mut app = test_app();
    app.held_turns.push_back(held_turn("first"));
    app.held_turns.push_back(held_turn("second"));

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

// Covers: compact-time prompts use the pending-input list, not held turns.
// Enter during idle compact is a follow-up; the queue-prompt chord is the same list.
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

// Covers: moving a queued follow-up into confirmation leaves no second queue
// owner that could send the same prompt after the confirmed copy.
// Owner: queued follow-up ownership
#[test]
fn confirmation_submission_takes_single_ownership_from_queue() {
    let mut app = test_app();
    app.pending.push_follow_up(super::super::QueuedPrompt {
        prompt: "model body".into(),
        display_prompt: "display body".into(),
        paste_segments: Vec::new(),
        media: vec![attached_document()],
    });

    let prompt = app.pending.pop_follow_up().expect("queued prompt");
    let submission = SendSubmission::turn(
        TurnPrompt::standard(prompt.prompt, prompt.display_prompt),
        prompt.media,
        prompt.paste_segments,
    );

    assert!(app.pending.queued_prompts().is_empty());
    assert_eq!(submission.turn_display(), Some("display body"));
}

// Covers: Unchanged/Failed compaction still starts the parked follow-up; cancel does not.
// Owner: compact follow-up drain
#[test]
fn unchanged_and_failed_compact_start_follow_ups_cancel_does_not() {
    use super::super::compaction_display::CompactionUiOutcome;

    assert!(CompactionUiOutcome::unchanged().starts_follow_ups());
    assert!(CompactionUiOutcome::failed("boom").starts_follow_ups());
    assert!(!CompactionUiOutcome::Cancelled.starts_follow_ups());
}

// Covers: an MCP hold is idle UI with ConnectingMcp activity, not a pending-input item.
// Owner: idle input hold/release
#[test]
fn mcp_hold_uses_connecting_activity_without_joining_pending_input() {
    let mut app = test_app();
    app.hold_turn(
        TurnPrompt::standard("hold-me".into(), "hold-me".into()),
        Vec::new(),
        Vec::new(),
    );

    assert_eq!(app.turn.session_ui(), SessionUiPhase::Idle);
    assert_eq!(
        app.activity_status(),
        Some(ActivityStatus::Parent {
            phase: ActivityPhase::ConnectingMcp,
            retry: None,
            background: crate::tui::activity::BackgroundCounts::default(),
        })
    );
    let lines = app.pending_input_lines(80);
    assert_eq!(lines.len(), 1);
    assert!(line_text(&lines[0]).contains("HOLD"));
    assert!(line_text(&lines[0]).contains("hold-me"));
    assert!(!line_text(&lines[0]).contains("pending input"));

    app.take_back_held_turn();
    assert_eq!(app.input_ui.text(), "hold-me");
    assert!(app.held_turns.is_empty());
    assert_eq!(app.activity_status(), None);
}

fn collapsible_paste() -> String {
    (1..=paste_burst::PASTE_COLLAPSE_MIN_LINES)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Covers: pasted /create-agent text must feed the model from expanded input,
// not the collapsed composer display.
// Owner: slash-command submission ownership
#[test]
fn pasted_create_agent_request_uses_expanded_model_input() {
    let mut app = test_app();
    let pasted = collapsible_paste();
    app.insert_input_text("/create-agent ");
    app.insert_pasted_input_text(&pasted);
    let expanded = app.expanded_input().trim().to_string();
    let display = app.input_ui.text().trim().to_string();
    let invocation = app.parse_input_command().unwrap().unwrap();
    let submission = app.take_command_submission(
        invocation,
        TurnPrompt::standard(expanded.clone(), display.clone()),
    );

    assert_eq!(slash_command_args(submission.model()).trim(), pasted);
    assert_eq!(submission.display(), display);
    assert_ne!(slash_command_args(submission.display()).trim(), pasted);
}

// Covers: `/agents create` execute path must drop the sub-action token from
// expanded model input, including a pasted request body.
// Owner: slash-command submission ownership
#[test]
fn pasted_agents_create_request_strips_create_token() {
    let mut app = test_app();
    let pasted = collapsible_paste();
    app.insert_input_text("/agents create ");
    app.insert_pasted_input_text(&pasted);
    let expanded = app.expanded_input().trim().to_string();
    let display = app.input_ui.text().trim().to_string();
    let invocation = app.parse_input_command().unwrap().unwrap();
    let turn = TurnPrompt::standard(expanded.clone(), display.clone());
    assert_eq!(
        super::super::agent_creator_command::create_agent_model_prompt(&invocation, &turn),
        format!("Create a new Rho agent through the guided workflow. User request: {pasted}")
    );
}

// Covers: Esc must restore TurnPrompt::command display, not the expanded
// model prompt held for MCP connect.
// Owner: idle input hold/release
#[test]
fn esc_restores_held_command_display_exactly() {
    let mut app = test_app();
    let display = "/create-agent a read-only reviewer";
    let model = format!(
        "Create a new Rho agent through the guided workflow. User request: {}",
        "a read-only reviewer"
    );
    app.hold_turn(
        TurnPrompt::command(model, display.to_owned()),
        Vec::new(),
        Vec::new(),
    );

    app.take_back_held_turn();
    assert_eq!(app.input_ui.text(), display);
    assert!(app.held_turns.is_empty());
}

// Covers: an MCP hold must not start a turn while a compact job holds the session.
// Owner: idle input hold/release
#[test]
fn mcp_holds_are_not_releasable_while_compacting() {
    let mut app = test_app();
    app.held_turns.push_back(held_turn("after mcp"));

    assert!(app.first_held_turn_is_releasable(false, false));
    assert!(!app.first_held_turn_is_releasable(false, true));
    assert!(!app.first_held_turn_is_releasable(true, false));
}
