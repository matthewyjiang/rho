use crossterm::event::MouseEventKind;
use pretty_assertions::assert_eq;
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use rho_providers::{
    auth::{browser::BrowserOpen, login_prompt::LoginPrompt},
    model::catalog::LoginTarget,
};

use super::*;
use crate::tui::{tests::test_app, ComposerMode, PendingLoginComposer};

fn step_text(step: SetupStep) -> Vec<String> {
    step_lines(step, 74)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

/// The rendered step list is what tells the user where they are: one row per
/// step, in order, each carrying its own marker. Earlier steps read as done,
/// the active one as current, and later ones as pending.
#[test]
fn the_step_list_renders_one_marked_row_per_step() {
    let cases = [
        (SetupStep::SignIn, [StepState::Current, StepState::Pending]),
        (
            SetupStep::ChooseModel,
            [StepState::Done, StepState::Current],
        ),
    ];

    for (step, states) in cases {
        let expected: Vec<String> = states
            .iter()
            .zip(STEP_LABELS)
            .map(|(state, label)| format!("{} {label}", state.marker()))
            .collect();
        assert_eq!(step_text(step), expected, "step rows at {step:?}");
    }
}

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
                BrowserOpen::Skipped,
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
            BrowserOpen::Skipped,
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
        app.login_copy_url_at_position(area, column, row).as_deref(),
        Some("https://auth.example/device?user_code=WD4E-T6MC")
    );

    let session = app.frame_context(area);
    pretty_assertions::assert_eq!(
        app.login_copy_url_at_position(
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

// Covers: first-run setup COPY hover uses the painted body origin, not session composer
// Owner: setup screen
#[test]
fn setup_copy_button_hover_follows_pointer() {
    let mut app = test_app();
    app.enter_setup(SetupStep::SignIn);
    app.input_ui
        .set_composer(ComposerMode::InteractivePending(pending_login()));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
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
    let on_row = origin.y.saturating_add(hit.row as u16);

    app.handle_mouse_event(MouseEventKind::Moved, column, on_row, &mut terminal)
        .unwrap();
    pretty_assertions::assert_eq!(app.input_ui.hovered_login_copy(), true);
    let hovered = app.setup_body_lines(origin.width as usize, origin.height);
    pretty_assertions::assert_eq!(
        hovered
            .get(hit.row)
            .and_then(|line| line.spans.last())
            .expect("copy span")
            .style,
        Theme::markdown_code_copy_button(/*hovered*/ true)
    );

    app.handle_mouse_event(MouseEventKind::Moved, origin.x, origin.y, &mut terminal)
        .unwrap();
    pretty_assertions::assert_eq!(app.input_ui.hovered_login_copy(), false);
    let unhovered = app.setup_body_lines(origin.width as usize, origin.height);
    pretty_assertions::assert_eq!(
        unhovered
            .get(hit.row)
            .and_then(|line| line.spans.last())
            .expect("copy span")
            .style,
        Theme::markdown_code_copy_button(/*hovered*/ false)
    );
}
