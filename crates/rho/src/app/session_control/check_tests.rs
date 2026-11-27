use std::collections::BTreeMap;

use pretty_assertions::{assert_eq, assert_ne};
use rho_protocol::{
    ApprovalAsk, ApprovalSource, InputAsker, Operation, PlanApprovalAsk, Question, Selection,
};

use super::*;

/// A questionnaire with the default title and one question, `q1`, asking
/// `prompt`.
fn ask(prompt: &str) -> QuestionAsk {
    QuestionAsk {
        title: "questionnaire".into(),
        reason: None,
        questions: vec![Question {
            id: "q1".into(),
            prompt: prompt.into(),
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

// Covers: an answer's approval text names each question's prompt, so an
// approval of `q1: Yes` to one question does not cover the same answer to
// another, as one a reloaded session asks under the same request id and
// the default title; a decline names the prompts it declines; two values
// read apart from one value that holds a comma, so an approval of one does
// not cover the other.
// Owner: session-control check, where the approval text is built.
#[test]
fn an_answers_approval_text_names_the_questions_prompts() {
    let params = AnswerParams {
        session: SessionId::new("s"),
        request: InputRequestId(0),
        answers: None,
        decline: false,
    };
    let answers = [
        QuestionAnswer::Answers {
            answers: BTreeMap::from([("q1".to_owned(), vec!["Yes".to_owned()])]),
        },
        QuestionAnswer::Decline,
    ];
    for answer in answers {
        assert_ne!(
            answer_text(&params, &ask("Deploy to staging?"), &answer),
            answer_text(&params, &ask("Delete the branch?"), &answer),
            "{answer:?}"
        );
    }
    let [two, one] = [vec!["A", "B"], vec!["A, B"]].map(|values| {
        let values = values.into_iter().map(str::to_owned).collect();
        let answer = QuestionAnswer::Answers {
            answers: BTreeMap::from([("q1".to_owned(), values)]),
        };
        answer_text(&params, &ask("Which?"), &answer)
    });
    assert_ne!(two, one);
}

// Covers: the check refuses, before anyone is asked to approve it, an
// answer to a request that is not open and to an approval or a plan
// approval, as the daemon would; it finds an open question otherwise.
// Owner: session-control check.
#[test]
fn only_an_open_question_takes_an_answer() {
    let request = |id, asked| InputRequest {
        id: InputRequestId(id),
        ask: asked,
        tool_call: None,
        timeout: None,
        asker: InputAsker::Turn,
    };
    let open = vec![
        request(1, Ask::Question(ask("Deploy to staging?"))),
        request(
            2,
            Ask::Approval(ApprovalAsk {
                source: ApprovalSource::Rho,
                operation: Operation::Network { url: None },
                reason: "network access".into(),
            }),
        ),
        request(
            3,
            Ask::PlanApproval(PlanApprovalAsk {
                plan: "plan".into(),
                modes: Vec::new(),
            }),
        ),
    ];
    let cases = [
        ("an open question", 1, Ok(ask("Deploy to staging?"))),
        ("an approval", 2, Err(Unanswerable::NotQuestion)),
        ("a plan approval", 3, Err(Unanswerable::NotQuestion)),
        ("a request not open", 4, Err(Unanswerable::NotOpen)),
    ];

    let found: Vec<_> = cases
        .iter()
        .map(|(case, id, _)| (*case, find_question(open.clone(), InputRequestId(*id))))
        .collect();
    let expected: Vec<_> = cases
        .into_iter()
        .map(|(case, _, expected)| (case, expected))
        .collect();
    assert_eq!(found, expected);
}
