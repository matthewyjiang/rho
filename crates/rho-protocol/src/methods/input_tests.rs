use pretty_assertions::assert_eq;
use serde_json::json;

use super::InputAnswer;
use crate::PermissionMode;

// Covers: a plan approval's answer without feedback is on the wire as it
// was before answers could carry any, so a client and a daemon on either
// side of that change understand each other; feedback rides beside it.
// Owner: protocol input answers
#[test]
fn plan_answers_keep_their_shape_without_feedback() {
    let cases = [
        (
            InputAnswer::KeepPlanning { feedback: None },
            json!({"kind": "keep_planning"}),
        ),
        (
            InputAnswer::ApprovePlan {
                mode: PermissionMode::Bypass,
                feedback: None,
            },
            json!({"kind": "approve_plan", "mode": "bypass"}),
        ),
        (
            InputAnswer::KeepPlanning {
                feedback: Some("smaller steps".into()),
            },
            json!({"kind": "keep_planning", "feedback": "smaller steps"}),
        ),
    ];
    for (answer, wire) in cases {
        assert_eq!(serde_json::to_value(&answer).unwrap(), wire);
        assert_eq!(serde_json::from_value::<InputAnswer>(wire).unwrap(), answer);
    }
}
