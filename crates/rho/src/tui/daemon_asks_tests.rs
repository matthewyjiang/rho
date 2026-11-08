use pretty_assertions::assert_eq;
use rho_protocol::{
    Ask, InputAnswer, InputRequest, InputRequestId, PermissionMode as WireMode, PlanApprovalAsk,
};
use rho_sdk::HostInputResponse;

use super::{plan_answer, DaemonPrompt};
use crate::tools::plan_exit::DECISION_QUESTION_ID;

fn plan_request(modes: Vec<WireMode>) -> InputRequest {
    InputRequest {
        id: InputRequestId(1),
        ask: Ask::PlanApproval(PlanApprovalAsk {
            plan: "1. do it".into(),
            modes,
        }),
        tool_call: None,
        timeout: None,
    }
}

// Covers: each choice the daemon plan approval offers answers as the mode it
// names, so a pick never approves into another permission mode; a mode this
// rho cannot name is not offered, and keeping on planning stays in plan mode.
// Owner: tui daemon input answers
#[test]
fn plan_approval_choices_answer_as_the_mode_they_name() {
    let cases = [
        (
            "every known mode, then keep planning",
            vec![WireMode::AllowEdits, WireMode::Supervised, WireMode::Bypass],
            vec![
                InputAnswer::ApprovePlan {
                    mode: WireMode::AllowEdits,
                },
                InputAnswer::ApprovePlan {
                    mode: WireMode::Supervised,
                },
                InputAnswer::ApprovePlan {
                    mode: WireMode::Bypass,
                },
                InputAnswer::KeepPlanning,
            ],
        ),
        (
            "a mode this rho does not know is left out",
            vec![WireMode::Unknown, WireMode::Auto],
            vec![
                InputAnswer::ApprovePlan {
                    mode: WireMode::Auto,
                },
                InputAnswer::KeepPlanning,
            ],
        ),
        (
            "no mode to approve into",
            Vec::new(),
            vec![InputAnswer::KeepPlanning],
        ),
    ];

    for (name, modes, expected) in cases {
        let Ok(DaemonPrompt::PlanApproval { form, modes }) = DaemonPrompt::of(&plan_request(modes))
        else {
            panic!("{name}: not a plan approval prompt");
        };
        let [question] = form.questions() else {
            panic!("{name}: expected the decision question alone");
        };
        let answers: Vec<_> = question
            .choices()
            .iter()
            .map(|choice| {
                let response =
                    HostInputResponse::new().answer(DECISION_QUESTION_ID, [choice.value()]);
                plan_answer(&response, &modes)
            })
            .collect();
        assert_eq!(answers, expected, "{name}");
    }
}
