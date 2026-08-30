use ratatui::layout::Rect;
use rho_providers::{
    auth::{browser::BrowserOpen, login_prompt::LoginPrompt},
    model::catalog::LoginTarget,
};

use super::super::{ComposerMode, PendingLoginComposer};
use super::login_composer_view;
use crate::tui::tests::test_app;

fn pending(prompt: LoginPrompt) -> PendingLoginComposer {
    PendingLoginComposer {
        target: LoginTarget {
            provider: "openai-codex".into(),
            auth: "codex".into(),
            label: "Codex".into(),
        },
        prompt,
    }
}

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn device_pending() -> PendingLoginComposer {
    pending(LoginPrompt::device_code(
        "https://auth.example/device",
        "WD4E-T6MC",
        Some("https://auth.example/device?user_code=WD4E-T6MC".into()),
        BrowserOpen::Skipped,
        "Visit this URL and enter the code.",
    ))
}

// Covers: pending login composer must show the authorize URL and code
// Owner: login presentation
#[test]
fn pending_login_composer_includes_url_and_code() {
    let pending = device_pending();
    let view = login_composer_view(&pending, 80);
    let texts: Vec<String> = view.lines.iter().map(line_text).collect();
    assert!(
        texts
            .iter()
            .any(|line| line.contains("https://auth.example/device")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|line| line.contains("WD4E-T6MC")),
        "{texts:?}"
    );
    assert!(view.copy_hit.is_some(), "{texts:?}");
}

// Covers: copy hit-testing uses the painted origin and composer_start, not line==row
// Owner: login presentation
#[test]
fn copy_hit_uses_painted_origin_for_scrolled_session_and_setup() {
    let pending = device_pending();
    let view = login_composer_view(&pending, 80);
    let hit = view.copy_hit.expect("copy button row");
    let copy_col = (hit.columns.start + 1) as u16;
    let url = "https://auth.example/device?user_code=WD4E-T6MC";

    let cases = [
        (
            "scrolled session",
            Rect {
                x: 0,
                y: 10,
                width: 80,
                height: 4,
            },
            2,
        ),
        (
            "setup body offset",
            Rect {
                x: 6,
                y: 9,
                width: 80,
                height: 12,
            },
            0,
        ),
    ];

    for (label, origin, start) in cases {
        let painted_row = origin
            .y
            .saturating_add(hit.row.saturating_sub(start) as u16);
        pretty_assertions::assert_eq!(
            hit.text_at(origin, start, origin.x + copy_col, painted_row),
            Some(url),
            "{label} painted origin"
        );
        pretty_assertions::assert_eq!(
            hit.text_at(origin, start, copy_col, hit.row as u16),
            None,
            "{label} naive line-as-row must miss"
        );
    }
}

// Covers: persisted transcript notices must not include the device code
// Owner: login presentation
#[test]
fn transcript_notice_omits_device_code() {
    let pending = device_pending();
    let lines = super::notice_lines("Codex", &pending.prompt);
    pretty_assertions::assert_eq!(
        lines,
        vec![
            "https://auth.example/device".to_string(),
            "Codex login pending".to_string(),
        ]
    );
}

// Covers: session composer copy uses layout.composer plus composer_start
// Owner: login presentation
#[test]
fn session_copy_button_hits_composer_layout() {
    let mut app = test_app();
    app.input_ui
        .set_composer(ComposerMode::InteractivePending(device_pending()));
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let ctx = app.frame_context(area);
    let hit = ctx.composer.copy_hit.expect("copy button");
    let column = ctx
        .layout
        .composer
        .x
        .saturating_add(hit.columns.start as u16 + 1);
    let row = ctx
        .layout
        .composer
        .y
        .saturating_add(hit.row.saturating_sub(ctx.layout.composer_start) as u16);
    pretty_assertions::assert_eq!(
        app.login_copy_url_at_position(area, column, row).as_deref(),
        Some("https://auth.example/device?user_code=WD4E-T6MC")
    );
}
