use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

use super::*;
use crate::{
    questionnaire::{QuestionnaireDefaultSelection, QuestionnaireQuestionKind},
    tui::{
        goal::GoalState,
        inline_shell::{InlineShellMode, PendingShellTask},
        questionnaire::{
            QuestionnaireComposer, QuestionnaireQuestion, QuestionnaireRequest,
            QuestionnaireResponseChannel,
        },
        tests::test_app,
        App, PickerItem, QueuedPrompt, UiPicker,
    },
};

fn follow_up(text: &str) -> QueuedPrompt {
    QueuedPrompt {
        prompt: text.into(),
        display_prompt: text.into(),
        paste_segments: Vec::new(),
        media: Vec::new(),
    }
}

fn pending_approval() -> rho_sdk::PendingApproval {
    let request = rho_sdk::ApprovalRequest::new(
        rho_sdk::CapabilityRequest::read_path(
            "/workspace/file",
            rho_sdk::PathScope::PrimaryWorkspace,
            rho_sdk::CapabilitySource::built_in_tool("read_file"),
        ),
        "approval required",
    );
    rho_sdk::PendingApproval::new(request).0
}

fn model_picker() -> UiPicker {
    UiPicker::models(
        "select model",
        vec![PickerItem {
            section: None,
            label: "model-a".into(),
            detail: None,
            preview: None,
            badge: None,
            value: "model-a".into(),
            selection_verb: None,
            allow_filter_completion: true,
        }],
    )
}

fn questionnaire_composer() -> QuestionnaireComposer {
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
    QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![QuestionnaireQuestion {
                id: "choice".into(),
                question: "choice?".into(),
                header: None,
                help: None,
                default: None,
                default_selection: QuestionnaireDefaultSelection::Selected,
                kind: QuestionnaireQuestionKind::Choice,
                required: true,
                choices: vec!["alpha".into(), "beta".into()],
                allow_other: false,
            }],
        },
        QuestionnaireResponseChannel::new(reply_tx),
    )
}

fn focus_pending_input(app: &mut App) {
    app.pending.push_follow_up(follow_up("queued"));
    assert!(app.handle_pending_input_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT,)));
}

// Covers: Esc policy must pick one owner, overlays before background shells,
// and idle/compact must not abort.
// Owner: tui running Esc policy
#[tokio::test]
async fn running_escape_action_picks_one_owner() {
    struct Case {
        name: &'static str,
        setup: fn(&mut App),
        expected: Option<RunningEscapeAction>,
    }

    let cases = [
        Case {
            name: "idle composer does not abort",
            setup: |_| {},
            expected: None,
        },
        Case {
            name: "empty provider turn aborts",
            setup: |app| app.begin_provider_turn_ui(),
            expected: Some(RunningEscapeAction::AbortTurn),
        },
        Case {
            name: "goal evaluation wait aborts",
            setup: |app| {
                app.goal = Some(GoalState::new("tests pass".into()));
                app.begin_cancellable_wait_ui();
            },
            expected: Some(RunningEscapeAction::AbortTurn),
        },
        Case {
            name: "limits overlay intercepts during a wait",
            setup: |app| {
                app.begin_cancellable_wait_ui();
                app.start_limits_command();
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "command palette intercepts",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.input_ui.set_text("/".into());
                app.input_ui.set_cursor(1);
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "pending-input panel intercepts",
            setup: |app| {
                app.begin_provider_turn_ui();
                focus_pending_input(app);
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "shell mode intercepts",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.input_ui
                    .set_shell_mode(Some(InlineShellMode::IncludeInContext));
            },
            expected: Some(RunningEscapeAction::ExitShellMode),
        },
        Case {
            name: "running inline shells intercept",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
            },
            expected: Some(RunningEscapeAction::CancelInlineShells),
        },
        Case {
            name: "pending inline shell + limits overlay",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
                app.start_limits_command();
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "pending inline shell + picker overlay",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
                app.input_ui
                    .set_composer(ComposerMode::Picker(model_picker()));
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "pending inline shell + questionnaire",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
                app.input_ui
                    .set_composer(ComposerMode::Questionnaire(questionnaire_composer()));
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "pending inline shell + command palette",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
                app.input_ui.set_text("/".into());
                app.input_ui.set_cursor(1);
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "pending inline shell + focused pending-input panel",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
                focus_pending_input(app);
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
        Case {
            name: "shell mode + focused pending-input panel",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.input_ui
                    .set_shell_mode(Some(InlineShellMode::IncludeInContext));
                focus_pending_input(app);
            },
            expected: Some(RunningEscapeAction::Overlay),
        },
    ];

    for case in cases {
        let mut app = test_app();
        (case.setup)(&mut app);
        assert_eq!(app.running_escape_action(), case.expected, "{}", case.name);
        let _ = app.cancel_inline_shells();
    }
}

// Covers: approval deny+abort stays first, including over background shells.
// Owner: tui running Esc policy
#[tokio::test]
async fn approval_escape_denies_and_aborts_first() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.open_approval(pending_approval()).await;
    assert_eq!(
        app.running_escape_action(),
        Some(RunningEscapeAction::DenyApprovalAndAbort)
    );

    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.pending_inline_shells
        .push(PendingShellTask::test_task("hello"));
    app.open_approval(pending_approval()).await;
    assert_eq!(
        app.running_escape_action(),
        Some(RunningEscapeAction::DenyApprovalAndAbort)
    );
    let _ = app.cancel_inline_shells();
}

// Covers: empty composer advertises abort only when Esc would abort.
// Owner: tui running Esc policy
#[tokio::test]
async fn empty_composer_abort_hint_follows_esc_policy() {
    struct Case {
        name: &'static str,
        setup: fn(&mut App),
        expected: bool,
    }

    let cases = [
        Case {
            name: "idle",
            setup: |_| {},
            expected: false,
        },
        Case {
            name: "provider turn",
            setup: |app| app.begin_provider_turn_ui(),
            expected: true,
        },
        Case {
            name: "cancellable wait",
            setup: |app| app.begin_cancellable_wait_ui(),
            expected: true,
        },
        Case {
            name: "compact",
            setup: |app| app.begin_compact_ui(),
            expected: false,
        },
        Case {
            name: "shell mode",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.input_ui
                    .set_shell_mode(Some(InlineShellMode::IncludeInContext));
            },
            expected: false,
        },
        Case {
            name: "pending inline shells",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
            },
            expected: false,
        },
    ];

    for case in cases {
        let mut app = test_app();
        (case.setup)(&mut app);
        assert_eq!(
            app.composer_shows_abort_hint(),
            case.expected,
            "{}",
            case.name
        );
        let _ = app.cancel_inline_shells();
    }
}

// Covers: a parent approval that displaces /side must put the overlay back
// so the next Enter stays in the aside, not the parent composer.
// Owner: tui approval composer restore
#[tokio::test]
async fn finishing_approval_restores_open_side_overlay() {
    struct Case {
        name: &'static str,
        setup: fn(&mut App),
        expected_side: bool,
    }

    let cases = [
        Case {
            name: "side open",
            setup: |app| app.open_side_chat(),
            expected_side: true,
        },
        Case {
            name: "side closed but retained",
            setup: |app| {
                app.open_side_chat();
                app.close_side_chat();
            },
            expected_side: false,
        },
        Case {
            name: "no side session",
            setup: |_| {},
            expected_side: false,
        },
    ];

    for case in cases {
        let mut app = test_app();
        (case.setup)(&mut app);
        app.open_approval(pending_approval()).await;
        assert!(
            matches!(app.input_ui.composer(), ComposerMode::Approval(_)),
            "{}",
            case.name
        );
        app.handle_approval_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 80, 24)
            .unwrap();
        pretty_assertions::assert_eq!(
            matches!(app.input_ui.composer(), ComposerMode::Side),
            case.expected_side,
            "{}",
            case.name
        );
    }
}
