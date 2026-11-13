use pretty_assertions::assert_eq;
use rho_sdk::model::{handoff::HandoffReport, ModelIdentity};
use rho_sdk::SessionId;

use super::{
    confirm_send_choice, SendOmissions, SendSubmission, ACTION_COMPACT_SEND, ACTION_DONT_SEND,
    ACTION_SEND, ACTION_SEND_SESSION,
};
use crate::tui::{tests::test_app, QueuedPrompt, TurnPrompt};

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

// Covers: the confirm-send modal always offers send/don't-send, only offers
// compaction when the conversation can be compacted, and keeps the established
// shortcuts: Don't send stays 2 without compaction, and session-wide consent
// comes last so it never takes over a key users already press
// Owner: send confirm gate
#[test]
fn confirm_send_options_depend_on_compact_availability() {
    let compactable = confirm_send_choice("xai/grok-4", &omissions(115), true).unwrap();
    assert_eq!(
        compactable
            .options
            .iter()
            .map(|option| (option.value.as_str(), option.shortcut))
            .collect::<Vec<_>>(),
        vec![
            (ACTION_SEND, '1'),
            (ACTION_COMPACT_SEND, '2'),
            (ACTION_DONT_SEND, '3'),
            (ACTION_SEND_SESSION, '4')
        ]
    );

    let not_compactable = confirm_send_choice("xai/grok-4", &omissions(115), false).unwrap();
    assert_eq!(
        not_compactable
            .options
            .iter()
            .map(|option| (option.value.as_str(), option.shortcut))
            .collect::<Vec<_>>(),
        vec![
            (ACTION_SEND, '1'),
            (ACTION_DONT_SEND, '2'),
            (ACTION_SEND_SESSION, '3')
        ]
    );
}

fn omissions_of(session: &SessionId, target: &ModelIdentity, blocks: &[u64]) -> SendOmissions {
    SendOmissions {
        session_id: session.clone(),
        target: target.clone(),
        blocks: blocks.iter().copied().collect(),
    }
}

// Covers: "Send for this session" consent covers a later send only while
// the session, target, and omitted blocks stay within what the user was
// warned about; compaction dropping blocks keeps it, anything new asks again.
// Owner: send confirm gate
#[test]
fn session_acknowledgement_covers_only_warned_omissions() {
    let session = SessionId::new();
    let target = ModelIdentity::new("openai-codex", "openai-responses", "gpt-target");
    let other_target = ModelIdentity::new("openai-codex", "openai-responses", "gpt-other");
    let acknowledged = omissions_of(&session, &target, &[1, 2, 3]);

    let cases = [
        (
            "nothing omitted",
            omissions_of(&session, &target, &[]),
            true,
        ),
        (
            "unchanged",
            omissions_of(&session, &target, &[1, 2, 3]),
            true,
        ),
        ("compacted", omissions_of(&session, &target, &[3]), true),
        (
            "new block",
            omissions_of(&session, &target, &[1, 2, 3, 4]),
            false,
        ),
        (
            "replaced block",
            omissions_of(&session, &target, &[1, 2, 9]),
            false,
        ),
        (
            "target changed",
            omissions_of(&session, &other_target, &[1, 2, 3]),
            false,
        ),
        (
            "session changed",
            omissions_of(&SessionId::new(), &target, &[1, 2, 3]),
            false,
        ),
    ];
    for (name, current, covered) in cases {
        assert_eq!(acknowledged.covers(&current), covered, "{name}");
    }
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
}

// Covers: a direct Don't-send restores the cancelled prompt to the composer
// instead of quietly scheduling it as pending work.
// Owner: send confirmation cancellation
#[test]
fn direct_cancellation_restores_without_queueing() {
    let mut app = test_app();
    app.cancel_send_submission(SendSubmission::turn(
        TurnPrompt::standard("model prompt".into(), "display prompt".into()),
        Vec::new(),
        Vec::new(),
    ));

    assert_eq!(app.input_ui.text(), "display prompt");
    assert!(app.pending.queued_prompts().is_empty());
}

// Covers: the consent given with "Compact, then send" on a daemon session
// goes with the parked prompt alone. A prompt queued ahead of it gets none,
// and taking the parked prompt off the queue any other way drops the consent,
// so a later prompt is never sent under it.
// Owner: send confirm gate
#[test]
fn compact_send_consent_goes_with_the_parked_prompt_alone() {
    use crate::tui::app_state::PendingWorkUi;

    let prompt = |text: &str| QueuedPrompt {
        prompt: text.into(),
        display_prompt: text.into(),
        paste_segments: Vec::new(),
        media: Vec::new(),
    };
    let parked = || {
        let mut pending = PendingWorkUi::default();
        let consent = omissions_of(
            &SessionId::new(),
            &ModelIdentity::new("openai-codex", "openai-responses", "gpt-target"),
            &[1],
        );
        pending.park_compact_send(prompt("parked"), consent);
        pending
    };
    // Each case leaves the queue's pops as (prompt, whether consent came).
    let pops = |mut pending: PendingWorkUi| {
        std::iter::from_fn(|| pending.pop_follow_up_with_consent())
            .map(|(prompt, consent)| (prompt.prompt, consent.is_some()))
            .collect::<Vec<_>>()
    };

    let sent = parked();
    let mut behind_steering = parked();
    behind_steering.push_follow_up_front(prompt("ahead"));
    let mut removed = parked();
    removed.remove_follow_up(0);
    removed.push_follow_up(prompt("later"));
    let mut cancelled = parked();
    let _ = cancelled.drain_queued_prompt_texts().count();
    cancelled.push_follow_up(prompt("parked"));

    assert_eq!(
        [
            pops(sent),
            pops(behind_steering),
            pops(removed),
            pops(cancelled),
        ],
        [
            vec![("parked".to_owned(), true)],
            vec![("ahead".to_owned(), false), ("parked".to_owned(), true)],
            vec![("later".to_owned(), false)],
            vec![("parked".to_owned(), false)],
        ]
    );
}
