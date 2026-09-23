use pretty_assertions::assert_eq;

use super::*;

const READY: SetupState = SetupState { signed_in: true };
const SIGNED_OUT: SetupState = SetupState { signed_in: false };

/// The header announces itself only when the session cannot run a turn. A
/// first launch says its welcome on the setup screen, so repeating it here
/// would greet the user twice.
#[test]
fn only_a_signed_out_session_adds_a_headline() {
    let cases = [(READY, false), (SIGNED_OUT, true)];

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
