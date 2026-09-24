use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;
use rho_providers::{auth::login_prompt::LoginPrompt, model::catalog::LoginTarget};

use super::*;
use crate::tui::{
    custom_provider_login::CustomHostStep, login::SecretInput, tests::test_app,
    text_input::TextInput, ComposerMode, PendingLoginComposer,
};

/// The content column stays centred and never runs past the terminal, so a
/// narrow pane keeps the copy on screen instead of clipping it away.
#[test]
fn the_content_column_is_centred_and_bounded() {
    let cases = [(30_u16, 30_u16), (74, 74), (200, CONTENT_WIDTH)];

    for (terminal_width, expected_width) in cases {
        let column = content_column(Rect {
            x: 0,
            y: 0,
            width: terminal_width,
            height: 24,
        });
        assert_eq!(column.width, expected_width, "width at {terminal_width}");
        assert!(
            column.x + column.width <= terminal_width,
            "column runs past the terminal at {terminal_width}"
        );
        assert_eq!(
            column.x,
            (terminal_width - expected_width) / 2,
            "left margin at {terminal_width}"
        );
    }
}

// Covers: first-run setup paints the login URL in the composer, not only transcript
// Owner: setup screen
#[test]
fn setup_body_shows_pending_login_url_and_code() {
    let mut app = test_app();
    app.enter_setup(SetupStep::SignIn);
    app.input_ui
        .set_composer(ComposerMode::InteractivePending(PendingLoginComposer {
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
        }));
    let lines: Vec<String> = app
        .setup_body_lines(80, 12)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("https://auth.example/device")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("WD4E-T6MC")),
        "{lines:?}"
    );
}

fn pending_login() -> PendingLoginComposer {
    PendingLoginComposer {
        target: LoginTarget {
            provider: "openai-codex".into(),
            auth: "codex".into(),
            label: "Codex".into(),
        },
        prompt: LoginPrompt::device_code(
            "https://auth.example/device",
            "WD4E-T6MC",
            Some("https://auth.example/device?user_code=WD4E-T6MC".into()),
            "Visit this URL and enter the code.",
        ),
    }
}

// Covers: first-run setup copy button must hit the painted body, not session composer
// Owner: setup screen
#[test]
fn setup_copy_button_hits_painted_origin_not_session_composer() {
    let mut app = test_app();
    app.enter_setup(SetupStep::SignIn);
    app.input_ui
        .set_composer(ComposerMode::InteractivePending(pending_login()));
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let origin = setup_composer_origin(area, SetupStep::SignIn);
    let frame = app.composer_frame(origin.width as usize, origin.height as usize);
    let hit = frame.copy_hit.expect("copy button");
    let column = origin.x.saturating_add(hit.columns.start as u16 + 1);
    let row = origin.y.saturating_add(hit.row as u16);
    pretty_assertions::assert_eq!(
        app.composer_copy_text_at(area, column, row).as_deref(),
        Some("https://auth.example/device?user_code=WD4E-T6MC")
    );

    let session = app.frame_context(area);
    pretty_assertions::assert_eq!(
        app.composer_copy_text_at(
            area,
            session
                .layout
                .composer
                .x
                .saturating_add(hit.columns.start as u16 + 1),
            session.layout.composer.y,
        ),
        None,
        "session composer rect must not steal the setup copy hit"
    );
}

fn composer_has_skip_hint(composer: &ComposerMode) -> bool {
    setup_skip_hint(composer).is_some()
}

// Covers: pending login must not also advertise Esc-to-skip
// Owner: setup screen
#[test]
fn setup_skip_footer_omits_while_login_pending() {
    let cases = [
        (
            "pending login",
            ComposerMode::InteractivePending(pending_login()),
            false,
        ),
        (
            "secret input",
            ComposerMode::SecretInput(SecretInput::new(pending_login().target)),
            false,
        ),
        (
            "custom host",
            ComposerMode::TextInput(TextInput::custom_host(
                CustomHostStep::Name {
                    api: rho_providers::provider::OpenAiCompatibleApi::ChatCompletions,
                },
                String::new(),
            )),
            false,
        ),
        ("plain composer", ComposerMode::Input, true),
        (
            "login picker",
            ComposerMode::Picker(crate::tui::provider_picker::login_group_picker()),
            true,
        ),
    ];
    for (label, composer, expected) in cases {
        assert_eq!(
            composer_has_skip_hint(&composer),
            expected,
            "skip footer at {label}"
        );
    }
}

// Covers: cancelling a pending login must restore the skip-setup footer with the picker
// Owner: setup screen
#[test]
fn cancelling_pending_login_restores_setup_skip_footer() {
    let mut app = test_app();
    app.enter_setup(SetupStep::SignIn);
    app.input_ui
        .set_composer(ComposerMode::InteractivePending(pending_login()));
    assert_eq!(
        composer_has_skip_hint(app.input_ui.composer()),
        false,
        "pending"
    );
    app.handle_interactive_pending_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        composer_has_skip_hint(app.input_ui.composer()),
        true,
        "after cancel"
    );
}
