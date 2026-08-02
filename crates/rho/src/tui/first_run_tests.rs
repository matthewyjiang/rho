use pretty_assertions::assert_eq;

use super::*;
use crate::tui::render::session_header_lines;

fn hint_texts(setup: SetupState) -> Vec<&'static str> {
    setup.hints().iter().map(|hint| hint.text).collect()
}

fn header_text(setup: SetupState) -> String {
    session_header_lines(None, setup, 80)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn setup_state_selects_headline_and_hints() {
    struct Case {
        name: &'static str,
        setup: SetupState,
        headline: Option<&'static str>,
        first_hint: &'static str,
    }

    let cases = [
        Case {
            name: "returning signed-in session",
            setup: SetupState {
                first_run: false,
                signed_in: true,
            },
            headline: None,
            first_hint: " shift+tab    Cycle reasoning level",
        },
        Case {
            name: "first run with credentials already stored",
            setup: SetupState {
                first_run: true,
                signed_in: true,
            },
            headline: Some(" Welcome to Rho. Type a prompt and press enter."),
            first_hint: " shift+tab    Cycle reasoning level",
        },
        Case {
            name: "first run with no credentials",
            setup: SetupState {
                first_run: true,
                signed_in: false,
            },
            headline: Some(" Welcome to Rho. Sign in to a provider to start."),
            first_hint: " /login       Sign in to a provider",
        },
        Case {
            name: "returning session after logout",
            setup: SetupState {
                first_run: false,
                signed_in: false,
            },
            headline: Some(" Not signed in. Rho needs a provider before it can answer."),
            first_hint: " /login       Sign in to a provider",
        },
    ];

    for case in cases {
        assert_eq!(
            case.setup.headline().map(|headline| headline.text),
            case.headline,
            "headline for {}",
            case.name
        );
        assert_eq!(
            hint_texts(case.setup).first().copied(),
            Some(case.first_hint),
            "leading hint for {}",
            case.name
        );
    }
}

#[test]
fn signed_out_header_leads_with_login() {
    let header = header_text(SetupState {
        first_run: true,
        signed_in: false,
    });
    assert!(
        header.contains("Sign in to a provider to start."),
        "welcome headline missing:\n{header}"
    );
    assert!(header.contains("/login"), "login hint missing:\n{header}");
    assert!(
        !header.contains("shift+tab"),
        "reasoning hint should wait until the session can run a turn:\n{header}"
    );
}

#[test]
fn ready_header_matches_the_returning_session_layout() {
    let header = header_text(SetupState::default());
    assert!(
        !header.contains("/login"),
        "a signed-in session should not be told to log in:\n{header}"
    );
    assert!(
        header.contains("shift+tab    Cycle reasoning level"),
        "reasoning hint missing:\n{header}"
    );
}

#[test]
fn login_hint_carries_the_next_step_tone() {
    let signed_out = SetupState {
        first_run: false,
        signed_in: false,
    };
    let next_steps: Vec<_> = signed_out
        .hints()
        .iter()
        .filter(|hint| hint.tone == HintTone::NextStep)
        .map(|hint| hint.text)
        .collect();
    assert_eq!(next_steps, vec![" /login       Sign in to a provider"]);

    let ready_next_steps = SetupState::default()
        .hints()
        .iter()
        .filter(|hint| hint.tone == HintTone::NextStep)
        .count();
    assert_eq!(ready_next_steps, 0);
}
