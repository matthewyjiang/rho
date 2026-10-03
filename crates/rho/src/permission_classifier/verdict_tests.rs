use pretty_assertions::assert_eq;

use super::{
    review_verdict, screen_verdict, ClassifierVerdict, ScreenVerdict, REVIEW_QUESTION,
    SCREEN_QUESTION,
};
use crate::decision::Answers;

fn answer(
    question_id: &'static str,
    option_id: &str,
    options: &'static [crate::decision::ChoiceOption],
) -> Answers {
    let option = options
        .iter()
        .find(|option| option.id == option_id)
        .unwrap();
    Answers::from([(question_id, option)])
}

// Covers: only the `allow` option skips review; anything else escalates.
// Owner: permission classifier screen answers.
#[test]
fn only_a_screen_allow_skips_review() {
    let options = SCREEN_QUESTION.options;
    let cases = [
        (
            answer(SCREEN_QUESTION.id, "allow", options),
            ScreenVerdict::Allow,
        ),
        (
            answer(SCREEN_QUESTION.id, "escalate", options),
            ScreenVerdict::Escalate,
        ),
        (Answers::new(), ScreenVerdict::Escalate),
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
