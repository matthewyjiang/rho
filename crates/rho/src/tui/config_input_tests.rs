use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rho_providers::{
    auth::{browser::BrowserOpen, login_prompt::LoginPrompt},
    model::catalog::LoginTarget,
};

use crate::tui::{
    exclusive_screen::ExclusiveOccupant, setup_screen::SetupStep, tests::test_app, ComposerMode,
    PendingLoginComposer,
};

fn pending() -> PendingLoginComposer {
    PendingLoginComposer {
        target: LoginTarget {
            provider: "openai-codex".into(),
            auth: "codex".into(),
            label: "Codex".into(),
        },
        prompt: LoginPrompt::device_code(
            "https://auth.example/device",
            "WD4E-T6MC",
            None,
            BrowserOpen::Skipped,
            "Visit this URL and enter the code.",
        ),
    }
}

// Covers: Ctrl+C during pending login must fall through so quit still works
// Owner: login composer input
#[test]
fn ctrl_c_during_pending_login_is_not_swallowed() {
    let mut app = test_app();
    app.input_ui
        .set_composer(ComposerMode::InteractivePending(pending()));
    pretty_assertions::assert_eq!(
        app.handle_interactive_pending_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        ))
        .unwrap(),
        false
    );
    pretty_assertions::assert_eq!(
        app.handle_interactive_pending_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE))
            .unwrap(),
        true
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelOccupant {
    Session,
    SignIn,
    ChooseModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelComposer {
    Input,
    Picker,
}

fn cancel_occupant(app: &crate::tui::App) -> CancelOccupant {
    match app.exclusive {
        ExclusiveOccupant::Setup(SetupStep::SignIn) => CancelOccupant::SignIn,
        ExclusiveOccupant::Setup(SetupStep::ChooseModel) => CancelOccupant::ChooseModel,
        ExclusiveOccupant::Session => CancelOccupant::Session,
        ExclusiveOccupant::Attach { .. } | ExclusiveOccupant::Peek { .. } => {
            panic!("unexpected exclusive occupant after login cancel")
        }
    }
}

fn cancel_composer(app: &crate::tui::App) -> CancelComposer {
    match app.input_ui.composer() {
        ComposerMode::Input => CancelComposer::Input,
        ComposerMode::Picker(_) => CancelComposer::Picker,
        other => panic!("unexpected composer after login cancel: {other:?}"),
    }
}

fn arm_pending_login(app: &mut crate::tui::App, occupant: CancelOccupant) {
    match occupant {
        CancelOccupant::Session => {
            app.exclusive = ExclusiveOccupant::Session;
        }
        CancelOccupant::SignIn => {
            app.exclusive = ExclusiveOccupant::Setup(SetupStep::SignIn);
        }
        CancelOccupant::ChooseModel => {
            app.exclusive = ExclusiveOccupant::Setup(SetupStep::ChooseModel);
        }
    }
    app.input_ui
        .set_composer(ComposerMode::InteractivePending(pending()));
}

// Covers: Esc on a pending login must restore setup's picker, not drop into session Input
// Owner: login composer input
#[test]
fn cancelling_pending_login_restores_setup_picker_or_session_input() {
    let cases = [
        (
            CancelOccupant::SignIn,
            CancelOccupant::SignIn,
            CancelComposer::Picker,
        ),
        (
            CancelOccupant::Session,
            CancelOccupant::Session,
            CancelComposer::Input,
        ),
        (
            CancelOccupant::ChooseModel,
            CancelOccupant::ChooseModel,
            CancelComposer::Picker,
        ),
    ];
    for (start, expected_occupant, expected_composer) in cases {
        let mut app = test_app();
        arm_pending_login(&mut app, start);
        app.handle_interactive_pending_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        pretty_assertions::assert_eq!(
            (cancel_occupant(&app), cancel_composer(&app)),
            (expected_occupant, expected_composer),
            "cancel from {start:?}"
        );
    }
}
