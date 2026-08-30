use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rho_providers::{
    auth::{browser::BrowserOpen, login_prompt::LoginPrompt},
    model::catalog::LoginTarget,
};

use crate::tui::{tests::test_app, ComposerMode, PendingLoginComposer};

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
