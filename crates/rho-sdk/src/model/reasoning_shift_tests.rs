use pretty_assertions::assert_eq;

use super::*;
use crate::ReasoningLevel::{High, Low, Medium};

fn shift(from: ReasoningLevel, to: ReasoningLevel) -> Message {
    Message::ReasoningShift(ReasoningShift { from, to })
}

// Covers: shifts are recorded only once the conversation started, consecutive
// changes collapse, and changing back removes the pending shift.
// Owner: SDK reasoning-shift history.
#[test]
fn record_appends_collapses_and_cancels_shifts() {
    let system = Message::System("prompt".into());
    let user = Message::user_text("hi");
    let cases = [
        (
            "before conversation",
            vec![system.clone()],
            (Low, High),
            vec![system.clone()],
            false,
        ),
        (
            "unchanged level",
            vec![user.clone()],
            (Low, Low),
            vec![user.clone()],
            false,
        ),
        (
            "first change",
            vec![user.clone()],
            (Low, High),
            vec![user.clone(), shift(Low, High)],
            true,
        ),
        (
            "consecutive change",
            vec![user.clone(), shift(Low, High)],
            (High, Medium),
            vec![user.clone(), shift(Low, Medium)],
            true,
        ),
        (
            "change back",
            vec![user.clone(), shift(Low, High)],
            (High, Low),
            vec![user.clone()],
            true,
        ),
    ];
    for (name, mut history, (from, to), expected, changed) in cases {
        assert_eq!(record(&mut history, from, to), changed, "{name}");
        assert_eq!(history, expected, "{name}");
    }
}
