use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rho_providers::{
    auth::login_prompt::LoginPrompt,
    model::{catalog::LoginTarget, provider_models::with_provider_models_cache_dir_for_tests},
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

fn test_app_with_selectable_model() -> crate::tui::App {
    let app = test_app();
    // OpenAI models come from a cache these unit tests do not populate.
    // xAI is a static-catalog provider, so a stored key makes models selectable.
    rho_providers::credentials::save_provider_api_key(
        app.credential_store.as_ref(),
        "xai",
        "xai-test",
    )
    .unwrap();
    app
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

// Covers: Esc on a login overlay restores that setup step's picker (not
// dismiss_setup_screen); with no selectable models, ChooseModel leaves
// setup and drops the overlay instead of leaving it stale.
// Owner: login composer input
#[test]
fn cancelling_login_overlay_restores_setup_picker_or_session_input() {
    // Cached Ollama/OpenAI lists make ChooseModel look populated on machines
    // that have already refreshed models. An empty cache matches CI and the
    // no-models fallback. xAI still works: it is a static catalog.
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        let occupants = [
            (
                CancelOccupant::SignIn,
                CancelOccupant::SignIn,
                CancelComposer::Picker,
                false,
            ),
            (
                CancelOccupant::Session,
                CancelOccupant::Session,
                CancelComposer::Input,
                false,
            ),
            (
                CancelOccupant::ChooseModel,
                CancelOccupant::ChooseModel,
                CancelComposer::Picker,
                true,
            ),
            // No selectable models: restore leaves setup and must drop the overlay.
            (
                CancelOccupant::ChooseModel,
                CancelOccupant::Session,
                CancelComposer::Input,
                false,
            ),
        ];
        let overlays = [
            CancelOverlay::Pending,
            CancelOverlay::Secret,
            CancelOverlay::CustomHost,
        ];
        for overlay in overlays {
            for (start, expected_occupant, expected_composer, selectable_model) in occupants {
                let mut app = if selectable_model {
                    test_app_with_selectable_model()
                } else {
                    test_app()
                };
                arm_login_overlay(&mut app, start, overlay);
                cancel_login_overlay(&mut app, overlay);
                pretty_assertions::assert_eq!(
                    (cancel_occupant(&app), cancel_composer(&app)),
                    (expected_occupant, expected_composer),
                    "cancel {overlay:?} from {start:?} selectable_model={selectable_model}"
                );
            }
        }
    });
}
