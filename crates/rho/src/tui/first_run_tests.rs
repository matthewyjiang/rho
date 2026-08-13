use pretty_assertions::assert_eq;

use super::*;
use crate::tui::render::session_header_lines;

const READY: SetupState = SetupState {
    signed_in: true,
    anthropic_usage_credits: false,
};
const SIGNED_OUT: SetupState = SetupState {
    signed_in: false,
    anthropic_usage_credits: false,
};
const ANTHROPIC_OAUTH: SetupState = SetupState {
    signed_in: true,
    anthropic_usage_credits: true,
};

fn header_lines(setup: SetupState) -> Vec<String> {
    session_header_lines(None, setup, 80)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

/// The header announces itself only when the session cannot run a turn. A
/// first launch says its welcome on the setup screen, so repeating it here
/// would greet the user twice.
#[test]
fn only_signed_out_or_usage_credit_sessions_add_a_headline() {
    let cases = [(READY, false), (SIGNED_OUT, true), (ANTHROPIC_OAUTH, true)];

    for (setup, expected) in cases {
        assert_eq!(
            setup.headline().is_some(),
            expected,
            "headline presence for {setup:?}"
        );
    }
}

/// Login is the only next step Rho pushes, and only while a session cannot run
/// a turn. A signed-in session must never be told to log in.
#[test]
fn login_is_the_next_step_exactly_while_signed_out() {
    let cases = [(READY, 0), (SIGNED_OUT, 1)];

    for (setup, expected) in cases {
        let next_steps = setup
            .hints()
            .iter()
            .filter(|hint| hint.tone == HintTone::NextStep)
            .count();
        assert_eq!(next_steps, expected, "next-step hints for {setup:?}");
        if expected > 0 {
            assert_eq!(
                setup.hints().first().map(|hint| hint.tone),
                Some(HintTone::NextStep),
                "the next step must lead the hint block for {setup:?}"
            );
        }
    }
}

/// The rendered header carries exactly the hints the state selected, in order,
/// so a state change cannot leave a stale hint block on screen.
#[test]
fn the_header_renders_the_hints_the_state_selected() {
    for setup in [READY, SIGNED_OUT, ANTHROPIC_OAUTH] {
        let rendered = header_lines(setup);
        let hints: Vec<&str> = setup.hints().iter().map(|hint| hint.text).collect();
        let rendered_hints: Vec<&str> = rendered
            .iter()
            .map(String::as_str)
            .filter(|line| hints.contains(line))
            .collect();
        assert_eq!(rendered_hints, hints, "rendered hint block for {setup:?}");

        let headline = setup
            .headline()
            .map(|headline| headline.content.into_owned());
        assert_eq!(
            rendered.iter().any(|line| Some(line) == headline.as_ref()),
            headline.is_some(),
            "rendered headline for {setup:?}"
        );
    }
}
