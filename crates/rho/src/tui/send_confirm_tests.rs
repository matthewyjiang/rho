use pretty_assertions::assert_eq;
use rho_sdk::model::handoff::HandoffReport;

use super::{
    confirm_send_choice, AuthorizedSendSubmission, CancellationSource, SendSubmission, TurnOrigin,
    ACTION_COMPACT_SEND, ACTION_DONT_SEND, ACTION_SEND,
};
use crate::tui::{
    goal, tests::test_app, ChatMedia, ChatTextDocument, ComposerAttachment, GoalState,
    QueuedPrompt, TurnPrompt,
};

fn omissions(count: usize) -> HandoffReport {
    HandoffReport {
        omitted_provider_context: count,
        omitted_kinds: if count == 0 {
            Vec::new()
        } else {
            vec!["openai_response_output_item".into()]
        },
    }
}

// Covers: the confirm-send modal always offers send/don't-send and only offers
// compaction when the conversation can be compacted
// Owner: send confirm gate
#[test]
fn confirm_send_options_depend_on_compact_availability() {
    let compactable = confirm_send_choice("xai/grok-4", &omissions(115), true).unwrap();
    assert_eq!(
        compactable
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec![ACTION_SEND, ACTION_COMPACT_SEND, ACTION_DONT_SEND]
    );

    let not_compactable = confirm_send_choice("xai/grok-4", &omissions(115), false).unwrap();
    assert_eq!(
        not_compactable
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec![ACTION_SEND, ACTION_DONT_SEND]
    );
}

// Covers: approval for one exact submission and model cannot become an ambient
// bypass after the active model changes.
// Owner: send confirmation authorization
#[test]
fn approval_is_scoped_to_the_confirmed_model_identity() {
    let confirmed = rho_sdk::model::ModelIdentity::new("openai", "responses", "gpt-5");
    let changed = rho_sdk::model::ModelIdentity::new("anthropic", "messages", "claude");
    let submission = SendSubmission::turn(
        TurnPrompt::standard("model body".into(), "display body".into()),
        Vec::new(),
        Vec::new(),
    )
    .approve_for(confirmed.clone());

    assert!(submission.is_approved_for(&confirmed));
    assert!(!submission.is_approved_for(&changed));
    assert_eq!(submission.turn_display(), Some("display body"));

    let submission = submission.after_compact();
    assert!(submission.is_approved_for(&confirmed));
    assert!(!submission.allows_auto_compact());
    let (_payload, authorization, allow_auto_compact) =
        AuthorizedSendSubmission(submission).into_authorized();
    assert!(authorization.matches(&confirmed));
    assert!(!authorization.matches(&changed));
    assert!(!allow_auto_compact);
}

fn document(name: &str, body: &str) -> ChatMedia {
    ChatMedia::TextDocument(ChatTextDocument {
        name: name.into(),
        mime: "text/plain".into(),
        body: body.into(),
        truncated: false,
        warnings: Vec::new(),
    })
}

fn queued(model: &str, display: &str, media: Vec<ChatMedia>) -> QueuedPrompt {
    QueuedPrompt {
        prompt: model.into(),
        display_prompt: display.into(),
        paste_segments: Vec::new(),
        media,
    }
}

// Covers: Esc during compact must preserve a draft and its attachments while
// parking the compact-owned submission intact without arming a send.
// Owner: compact-send cancellation ownership
#[test]
fn compact_send_cancellation_parks_on_composer_collision() {
    let mut app = test_app();
    let draft_media = document("draft.txt", "draft attachment");
    let compact_media = document("compact.txt", "compact attachment");
    app.input_ui.set_text("typed during compact".into());
    app.input_ui
        .push_ready_attachment(draft_media.clone(), None);

    app.apply_turn_cancellation(
        TurnOrigin::User,
        queued(
            "exact model prompt",
            "submitted prompt",
            vec![compact_media.clone()],
        ),
        CancellationSource::Compact,
    );

    assert_eq!(app.input_ui.text(), "typed during compact");
    assert_eq!(
        app.input_ui.attachments(),
        vec![ComposerAttachment::Ready(draft_media)]
    );
    assert_eq!(
        app.pending.queued_prompts().front(),
        Some(&queued(
            "exact model prompt",
            "submitted prompt",
            vec![compact_media]
        ))
    );
    assert!(app.start_follow_ups.is_none());

    let mut empty = test_app();
    let restored_media = document("restored.txt", "restored attachment");
    empty.apply_turn_cancellation(
        TurnOrigin::User,
        queued(
            "restored model prompt",
            "restored prompt",
            vec![restored_media.clone()],
        ),
        CancellationSource::Compact,
    );
    assert_eq!(empty.input_ui.text(), "restored prompt");
    assert_eq!(
        empty.input_ui.attachments(),
        vec![ComposerAttachment::Ready(restored_media)]
    );
    assert!(empty.pending.queued_prompts().is_empty());
}

// Covers: a direct Don't-send restores the cancelled prompt to the composer
// instead of quietly scheduling it as pending work.
// Owner: send confirmation cancellation
#[test]
fn direct_cancellation_restores_without_queueing() {
    let mut app = test_app();
    app.apply_turn_cancellation(
        TurnOrigin::User,
        queued("model prompt", "display prompt", Vec::new()),
        CancellationSource::DirectConfirmation,
    );

    assert_eq!(app.input_ui.text(), "display prompt");
    assert!(app.pending.queued_prompts().is_empty());
    assert!(app.start_follow_ups.is_none());
}

fn blocked_goal() -> GoalState {
    let mut goal = GoalState::new("release is published".into());
    goal.record_evaluation(&goal::GoalEvaluation::Blocked {
        reason: "waiting for user".into(),
        pending_steps: vec![goal::HumanStep {
            action: "publish release".into(),
            reason: "requires user credentials".into(),
        }],
    });
    goal
}

// Covers: goal-owned confirmation cancellation applies lifecycle state rather
// than treating synthetic display text as ordinary composer input.
// Owner: goal send cancellation policy
#[test]
fn goal_submission_cancellation_applies_typed_state_transition() {
    let mut initial = test_app();
    initial.goal = Some(GoalState::new("tests pass".into()));
    initial.apply_turn_cancellation(
        TurnOrigin::InitialGoal,
        queued("synthetic initial prompt", "/goal tests pass", Vec::new()),
        CancellationSource::DirectConfirmation,
    );
    assert!(initial.goal.is_none());
    assert_eq!(initial.input_ui.text(), "/goal tests pass");

    let mut resume = test_app();
    resume.goal = Some(blocked_goal());
    assert!(resume.goal.as_mut().unwrap().begin_verification());
    resume.apply_turn_cancellation(
        TurnOrigin::GoalResume,
        queued("synthetic verification", "/goal resume", Vec::new()),
        CancellationSource::DirectConfirmation,
    );
    resume.goal.as_mut().unwrap().complete_verification();
    assert!(resume.goal.as_ref().unwrap().is_blocked());
    assert_eq!(resume.input_ui.text(), "/goal resume");

    let mut continuation = test_app();
    continuation.goal = Some(GoalState::new("tests pass".into()));
    continuation.apply_turn_cancellation(
        TurnOrigin::GoalContinuation,
        queued(
            "synthetic continuation prompt",
            "continuing active goal",
            Vec::new(),
        ),
        CancellationSource::DirectConfirmation,
    );
    assert!(continuation.input_ui.text().is_empty());
    assert!(continuation.pending.queued_prompts().is_empty());

    let mut colliding_resume = test_app();
    colliding_resume.goal = Some(blocked_goal());
    assert!(colliding_resume.goal.as_mut().unwrap().begin_verification());
    colliding_resume.input_ui.set_text("new draft".into());
    colliding_resume.apply_turn_cancellation(
        TurnOrigin::GoalResume,
        queued("synthetic verification", "/goal resume", Vec::new()),
        CancellationSource::Compact,
    );
    assert_eq!(colliding_resume.input_ui.text(), "new draft");
    assert!(colliding_resume.pending.queued_prompts().is_empty());
    colliding_resume
        .goal
        .as_mut()
        .unwrap()
        .complete_verification();
    assert!(colliding_resume.goal.as_ref().unwrap().is_blocked());
}
