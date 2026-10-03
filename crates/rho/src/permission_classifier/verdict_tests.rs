use pretty_assertions::assert_eq;

use super::{
    review_verdict, screen_verdict, verdict::SCREEN_ALLOW_THRESHOLD, ClassifierVerdict,
    ScreenVerdict, REVIEW_QUESTION, SCREEN_QUESTION,
};
use crate::decision::{Answer, Answers};

fn answer(
    question_id: &'static str,
    option_id: &str,
    options: &'static [crate::decision::ChoiceOption],
) -> Answers {
    answer_with_allow_probability(question_id, option_id, options, /*allow*/ None)
}

fn answer_with_allow_probability(
    question_id: &'static str,
    option_id: &str,
    options: &'static [crate::decision::ChoiceOption],
    allow: Option<f64>,
) -> Answers {
    let option = options
        .iter()
        .find(|option| option.id == option_id)
        .unwrap();
    let probabilities = allow.map(|allow| [("allow", allow)].into());
    Answers::from([(
        question_id,
        Answer {
            option,
            probabilities,
        },
    )])
}

// Covers: only the `allow` option skips review, and from a model that reports
// probabilities only at the threshold; a text model's allow, which has none,
// still allows. Anything else escalates.
// Owner: permission classifier screen answers.
#[test]
fn only_a_screen_allow_skips_review() {
    let (id, options) = (SCREEN_QUESTION.id, SCREEN_QUESTION.options);
    let cases = [
        (answer(id, "allow", options), ScreenVerdict::Allow),
        (answer(id, "escalate", options), ScreenVerdict::Escalate),
        (Answers::new(), ScreenVerdict::Escalate),
        (
            answer_with_allow_probability(id, "allow", options, Some(SCREEN_ALLOW_THRESHOLD)),
            ScreenVerdict::Allow,
        ),
        (
            answer_with_allow_probability(id, "allow", options, Some(0.96)),
            ScreenVerdict::Escalate,
        ),
        (
            answer_with_allow_probability(id, "escalate", options, Some(1.0)),
            ScreenVerdict::Escalate,
        ),
    ];

    for (answers, expected) in cases {
        assert_eq!(screen_verdict(&answers), expected, "{answers:?}");
    }
}

// Covers: only the `allow` option allows; every other option denies with its
// own description as the agent-facing reason, so no model-written text
// reaches the agent. A missing answer is an error, which fails closed.
// Owner: permission classifier review answers.
#[test]
fn every_review_option_but_allow_denies_with_its_description() {
    let verdicts: Vec<_> = REVIEW_QUESTION
        .options
        .iter()
        .map(|option| {
            let answers = answer(REVIEW_QUESTION.id, option.id, REVIEW_QUESTION.options);
            (option.id, review_verdict(&answers).unwrap())
        })
        .collect();

    let expected: Vec<_> = REVIEW_QUESTION
        .options
        .iter()
        .map(|option| {
            let verdict = match option.id {
                "allow" => ClassifierVerdict::Allow,
                _ => ClassifierVerdict::Deny {
                    reason: option.description.to_owned(),
                },
            };
            (option.id, verdict)
        })
        .collect();
    assert_eq!(verdicts, expected);
    assert!(review_verdict(&Answers::new()).is_err());
}
