use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use ratatui::{backend::TestBackend, Terminal};

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
        App, PickerAction, PickerItem, QueuedPrompt, StreamControl, UiPicker,
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

fn esc_event() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
}

async fn running_esc_control(app: &mut App) -> StreamControl {
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    let interrupt_requested = AtomicBool::new(false);
    let tool_call_active = AtomicBool::new(false);
    let mut open_editor = false;
    let control = match app
        .route_running_terminal_event(
            esc_event(),
            &mut terminal,
            &interrupt_requested,
            &tool_call_active,
            &mut open_editor,
        )
        .await
    {
        Ok(control) => control,
        Err(_) => panic!("running Esc should route"),
    };
    assert!(!open_editor, "Esc must not open the external editor");
    if matches!(control, StreamControl::Interrupt) {
        assert!(interrupt_requested.load(Ordering::SeqCst));
    } else {
        assert!(!interrupt_requested.load(Ordering::SeqCst));
    }
    control
}

fn model_picker() -> UiPicker {
    UiPicker::new(
        "select model",
        vec![PickerItem {
            section: None,
            label: "model-a".into(),
            detail: None,
            preview: None,
            badge: None,
            value: "model-a".into(),
            selection_verb: None,
        }],
        PickerAction::SelectModel,
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

// Covers: Esc on the composer must interrupt live operations, including
// goal-between-turns waits, while overlays/shells intercept without abort.
// Owner: tui running Esc routing
#[tokio::test]
async fn running_escape_routes_interrupt_without_overlay_intercept() {
    struct Case {
        name: &'static str,
        setup: fn(&mut App),
        expected_action: RunningEscapeAction,
        expected_control: StreamControl,
    }

    let cases = [
        Case {
            name: "idle composer does not abort",
            setup: |_| {},
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
        },
        Case {
            name: "empty provider turn aborts",
            setup: |app| app.begin_provider_turn_ui(),
            expected_action: RunningEscapeAction::AbortTurn,
            expected_control: StreamControl::Interrupt,
        },
        Case {
            name: "goal evaluation wait aborts",
            setup: |app| {
                app.goal = Some(GoalState::new("tests pass".into()));
                app.begin_cancellable_wait_ui();
                app.turn.start_loading();
            },
            expected_action: RunningEscapeAction::AbortTurn,
            expected_control: StreamControl::Interrupt,
        },
        Case {
            name: "limits overlay intercepts during a wait",
            setup: |app| {
                app.begin_cancellable_wait_ui();
                app.start_limits_command();
            },
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
        },
        Case {
            name: "command palette intercepts",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.input_ui.set_text("/".into());
                app.input_ui.set_cursor(1);
            },
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
        },
        Case {
            name: "pending-input panel intercepts",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending.push_follow_up(follow_up("queued"));
                assert!(app.handle_pending_input_key(KeyEvent::new(
                    KeyCode::Char('q'),
                    KeyModifiers::ALT,
                )));
            },
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
        },
        Case {
            name: "shell mode intercepts",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.input_ui
                    .set_shell_mode(Some(InlineShellMode::IncludeInContext));
            },
            expected_action: RunningEscapeAction::ExitShellMode,
            expected_control: StreamControl::Continue,
        },
        Case {
            name: "running inline shells intercept",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
            },
            expected_action: RunningEscapeAction::CancelInlineShells,
            expected_control: StreamControl::Continue,
        },
    ];

    for case in cases {
        let mut app = test_app();
        (case.setup)(&mut app);
        assert_eq!(
            app.running_escape_action(),
            case.expected_action,
            "{}",
            case.name
        );
        assert_eq!(
            running_esc_control(&mut app).await,
            case.expected_control,
            "{}",
            case.name
        );
        let _ = app.cancel_inline_shells();
    }
}

// Covers: visible/focused overlays and modal composers own Esc ahead of
// background pending inline shells or shell mode; approval deny+abort stays first.
// Owner: tui running Esc routing
#[tokio::test]
async fn running_escape_gives_overlays_priority_over_background_shells() {
    struct Case {
        name: &'static str,
        setup: fn(&mut App),
        expected_action: RunningEscapeAction,
        expected_control: StreamControl,
        after: fn(&mut App),
    }

    let cases = [
        Case {
            name: "pending inline shell + limits overlay",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
                app.start_limits_command();
            },
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
            after: |app| {
                assert!(!app.limits_overlay_open(), "limits overlay should close");
                assert_eq!(app.pending_inline_shells.len(), 1);
            },
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
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
            after: |app| {
                assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
                assert_eq!(app.pending_inline_shells.len(), 1);
            },
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
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
            after: |app| {
                assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
                assert_eq!(app.pending_inline_shells.len(), 1);
            },
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
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
            after: |app| {
                assert!(!app.command_palette_visible());
                assert_eq!(app.pending_inline_shells.len(), 1);
            },
        },
        Case {
            name: "pending inline shell + focused pending-input panel",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.pending_inline_shells
                    .push(PendingShellTask::test_task("hello"));
                app.pending.push_follow_up(follow_up("queued"));
                assert!(app.handle_pending_input_key(KeyEvent::new(
                    KeyCode::Char('q'),
                    KeyModifiers::ALT,
                )));
                assert!(app.pending_input_focused());
            },
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
            after: |app| {
                assert!(!app.pending_input_focused());
                assert_eq!(app.pending_inline_shells.len(), 1);
            },
        },
        Case {
            name: "shell mode + focused pending-input panel",
            setup: |app| {
                app.begin_provider_turn_ui();
                app.input_ui
                    .set_shell_mode(Some(InlineShellMode::IncludeInContext));
                app.pending.push_follow_up(follow_up("queued"));
                assert!(app.handle_pending_input_key(KeyEvent::new(
                    KeyCode::Char('q'),
                    KeyModifiers::ALT,
                )));
                assert!(app.pending_input_focused());
            },
            expected_action: RunningEscapeAction::Overlay,
            expected_control: StreamControl::Continue,
            after: |app| {
                assert!(!app.pending_input_focused());
                assert_eq!(
                    app.input_ui.shell_mode(),
                    Some(InlineShellMode::IncludeInContext)
                );
            },
        },
    ];

    for case in cases {
        let mut app = test_app();
        (case.setup)(&mut app);
        assert_eq!(
            app.running_escape_action(),
            case.expected_action,
            "{}",
            case.name
        );
        assert_eq!(
            running_esc_control(&mut app).await,
            case.expected_control,
            "{}",
            case.name
        );
        (case.after)(&mut app);
        let _ = app.cancel_inline_shells();
    }
}

// Covers: Esc during a goal wait must interrupt through the same handler
// callers such as continue_goal match on StreamControl::Interrupt.
// Owner: tui goal-between-turns cancellation
#[tokio::test]
async fn goal_wait_escape_interrupts_and_clears_like_continue_goal() {
    let mut app = test_app();
    app.goal = Some(GoalState::new("tests pass".into()));
    app.begin_cancellable_wait_ui();
    app.turn.start_loading();

    let control = running_esc_control(&mut app).await;
    assert_eq!(control, StreamControl::Interrupt);
    if matches!(control, StreamControl::Interrupt) {
        app.clear_goal();
    }
    assert!(app.goal.is_none());
    assert_eq!(app.status(), "goal cleared");
    let _ = app.cancel_inline_shells();
}

#[tokio::test]
async fn approval_escape_denies_and_aborts() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.open_approval(pending_approval()).await;
    assert_eq!(
        app.running_escape_action(),
        RunningEscapeAction::DenyApprovalAndAbort
    );
    assert_eq!(
        running_esc_control(&mut app).await,
        StreamControl::Interrupt
    );
    let _ = app.cancel_inline_shells();
}

// Covers: approval Esc still denies and aborts even when a background inline
// shell is pending, because that path is intentionally first.
// Owner: tui running Esc routing
#[tokio::test]
async fn approval_escape_denies_and_aborts_over_pending_inline_shells() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.pending_inline_shells
        .push(PendingShellTask::test_task("hello"));
    app.open_approval(pending_approval()).await;
    assert_eq!(
        app.running_escape_action(),
        RunningEscapeAction::DenyApprovalAndAbort
    );
    assert_eq!(
        running_esc_control(&mut app).await,
        StreamControl::Interrupt
    );
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert!(app.pending_inline_shells.is_empty());
}
