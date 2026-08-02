use pretty_assertions::assert_eq;

use super::*;

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
