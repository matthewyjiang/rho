use pretty_assertions::assert_eq;

use super::{process_outcome, CommandExit, NodeTerminalState};

// Covers: an incomplete cleanup must not turn a successful child exit into a
// successful workflow node, while existing non-success states stay typed.
// Owner: workflow command outcome mapping.
#[test]
fn incomplete_cleanup_cannot_produce_success() {
    let cases = [
        (
            CommandExit::Code { code: 0 },
            true,
            NodeTerminalState::Failure,
        ),
        (
            CommandExit::Cancellation,
            true,
            NodeTerminalState::Cancellation,
        ),
        (
            CommandExit::Code { code: 0 },
            false,
            NodeTerminalState::Success,
        ),
    ];

    for (exit, cleanup_incomplete, expected) in cases {
        assert_eq!(process_outcome(&exit, cleanup_incomplete), expected);
    }
}
