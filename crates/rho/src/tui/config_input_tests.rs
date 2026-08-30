use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rho_providers::{
    auth::{browser::BrowserOpen, login_prompt::LoginPrompt},
    model::catalog::LoginTarget,
};

use crate::tui::{
    custom_provider_login::CustomHostStep, exclusive_screen::ExclusiveOccupant, login::SecretInput,
    setup_screen::SetupStep, tests::test_app, text_input::TextInput, ComposerMode,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelOverlay {
    Pending,
    Secret,
    CustomHost,
}

fn set_cancel_occupant(app: &mut crate::tui::App, occupant: CancelOccupant) {
    app.exclusive = match occupant {
        CancelOccupant::Session => ExclusiveOccupant::Session,
        CancelOccupant::SignIn => ExclusiveOccupant::Setup(SetupStep::SignIn),
        CancelOccupant::ChooseModel => ExclusiveOccupant::Setup(SetupStep::ChooseModel),
    };
}

fn arm_login_overlay(app: &mut crate::tui::App, occupant: CancelOccupant, overlay: CancelOverlay) {
    set_cancel_occupant(app, occupant);
    match overlay {
        CancelOverlay::Pending => {
            app.input_ui
                .set_composer(ComposerMode::InteractivePending(pending()));
        }
        CancelOverlay::Secret => {
            app.input_ui
                .set_composer(ComposerMode::SecretInput(SecretInput::new(
                    pending().target,
                )));
        }
        CancelOverlay::CustomHost => {
            app.input_ui
                .set_composer(ComposerMode::TextInput(TextInput::custom_host(
                    CustomHostStep::Name {
                        api: rho_providers::provider::OpenAiCompatibleApi::ChatCompletions,
                    },
                    String::new(),
                )));
        }
    }
}

fn cancel_login_overlay(app: &mut crate::tui::App, overlay: CancelOverlay) {
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    match overlay {
        CancelOverlay::Pending => {
            app.handle_interactive_pending_key(esc).unwrap();
        }
        CancelOverlay::Secret => {
            app.apply_secret_key(esc);
        }
        CancelOverlay::CustomHost => {
            app.handle_text_input_key(esc).unwrap();
        }
    }
}

// Covers: Esc on a login overlay must restore setup's picker, not drop into session Input
// Owner: login composer input
#[test]
fn cancelling_login_overlay_restores_setup_picker_or_session_input() {
    let occupants = [
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
    let overlays = [
        CancelOverlay::Pending,
        CancelOverlay::Secret,
        CancelOverlay::CustomHost,
    ];
    for overlay in overlays {
        for (start, expected_occupant, expected_composer) in occupants {
            let mut app = test_app();
            arm_login_overlay(&mut app, start, overlay);
            cancel_login_overlay(&mut app, overlay);
            pretty_assertions::assert_eq!(
                (cancel_occupant(&app), cancel_composer(&app)),
                (expected_occupant, expected_composer),
                "cancel {overlay:?} from {start:?}"
            );
        }
    }
}
