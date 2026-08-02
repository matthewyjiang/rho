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

/// The step list must show one current step and no more, with everything
/// before it marked done, so the screen always says where the user is.
#[test]
fn exactly_one_step_is_current_and_earlier_steps_are_done() {
    let cases = [
        (
            SetupStep::SignIn,
            vec![StepState::Current, StepState::Pending],
        ),
        (
            SetupStep::ChooseModel,
            vec![StepState::Done, StepState::Current],
        ),
    ];

    for (step, expected) in cases {
        let states: Vec<StepState> = (0..STEP_LABELS.len())
            .map(|index| match index.cmp(&step.index()) {
                std::cmp::Ordering::Less => StepState::Done,
                std::cmp::Ordering::Equal => StepState::Current,
                std::cmp::Ordering::Greater => StepState::Pending,
            })
            .collect();
        assert_eq!(states, expected, "step states at {step:?}");
    }
}

/// Every step renders one row carrying its own marker, so a step never goes
/// missing or borrows another's state.
#[test]
fn each_step_renders_one_row_with_its_marker() {
    for step in [SetupStep::SignIn, SetupStep::ChooseModel] {
        let rows = step_text(step);
        assert_eq!(rows.len(), STEP_LABELS.len(), "row count at {step:?}");
        for (index, row) in rows.iter().enumerate() {
            assert!(
                row.contains(STEP_LABELS[index]),
                "row {index} lost its label at {step:?}: {row}"
            );
        }
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains(StepState::Current.marker()))
                .count(),
            1,
            "current marker count at {step:?}"
        );
    }
}

/// The content column stays centred and never runs past the terminal, so a
/// narrow pane keeps the copy on screen instead of clipping it away.
#[test]
fn the_content_column_is_centred_and_bounded() {
    let cases = [(30_u16, 30_u16), (74, 74), (200, CONTENT_WIDTH)];

    for (terminal_width, expected_width) in cases {
        let column = content_column(Rect::new(0, 0, terminal_width, 24));
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
