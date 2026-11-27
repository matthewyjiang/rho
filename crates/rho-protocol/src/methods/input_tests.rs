use pretty_assertions::assert_eq;
use serde_json::json;

use super::{InputAnswer, Question, QuestionAsk, Selection};
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

fn ask(reason: Option<&str>) -> QuestionAsk {
    QuestionAsk {
        title: "Deploy".into(),
        reason: reason.map(str::to_owned),
        questions: vec![Question {
            id: "target".into(),
            prompt: "Where to?".into(),
            header: None,
            choices: Vec::new(),
            selection: Selection::One,
            allow_other: true,
            help: None,
            default: None,
            required: true,
        }],
    }
}

// Covers: a question's `reason` rides beside its title and reads back as
// written, and a question without one, as every question from a daemon
// that predates the field, leaves it out and reads as having none.
// Owner: protocol input requests
#[test]
fn a_questions_reason_is_optional_on_the_wire() {
    let question = json!({
        "id": "target",
        "prompt": "Where to?",
        "choices": [],
        "selection": "one",
        "allow_other": true,
        "required": true,
    });
    let cases = [
        (
            ask(Some("staging is down")),
            json!({"title": "Deploy", "reason": "staging is down", "questions": [question]}),
        ),
        (
            ask(None),
            json!({"title": "Deploy", "questions": [question]}),
        ),
    ];
    for (ask, wire) in cases {
        assert_eq!(serde_json::to_value(&ask).unwrap(), wire);
        assert_eq!(serde_json::from_value::<QuestionAsk>(wire).unwrap(), ask);
    }
}
