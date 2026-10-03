use pretty_assertions::assert_eq;

use super::{
    review_verdict, screen_verdict, verdict::SCREEN_ALLOW_THRESHOLD, ClassifierVerdict,
    ScreenVerdict, REVIEW_QUESTION, SCREEN_QUESTION,
};
use rho_sdk::decision::{Answer, ChoiceAnswer, ChoiceOption, Question, QuestionKind};

fn options(question: &Question<'static>) -> &'static [ChoiceOption<'static>] {
    match question.kind {
        QuestionKind::Choice(options) => options,
        QuestionKind::Noul(_) | QuestionKind::Score(_) => panic!("not a choice question"),
    }
}

fn answer(question: &Question<'static>, option_id: &str) -> Vec<Answer> {
    answer_with_allow_probability(question, option_id, /*allow*/ None)
}

/// An answer choosing `option_id`, with probabilities when `allow` is set:
/// `allow` for the `allow` option and the rest spread over the others.
fn answer_with_allow_probability(
    question: &Question<'static>,
    option_id: &str,
    allow: Option<f64>,
) -> Vec<Answer> {
    let options = options(question);
    let option = options
        .iter()
        .position(|option| option.id == option_id)
        .unwrap();
    let answer = match allow {
        None => ChoiceAnswer::from_option(option),
        Some(allow) => {
            let rest = (1.0 - allow) / (options.len() - 1) as f64;
            let probabilities = options
                .iter()
                .map(|option| if option.id == "allow" { allow } else { rest })
                .collect();
            ChoiceAnswer::from_probabilities(option, probabilities).unwrap()
        }
    };
    vec![Answer::Choice(answer)]
}

// Covers: only the `allow` option skips review, and from a model that reports
// probabilities only at the threshold; a text model's allow, which has none,
// still allows. Anything else escalates.
// Owner: permission classifier screen answers.
#[test]
fn only_a_screen_allow_skips_review() {
    let cases = [
        (answer(&SCREEN_QUESTION, "allow"), ScreenVerdict::Allow),
        (
            answer(&SCREEN_QUESTION, "escalate"),
            ScreenVerdict::Escalate,
        ),
        (Vec::new(), ScreenVerdict::Escalate),
        (
            answer_with_allow_probability(&SCREEN_QUESTION, "allow", Some(SCREEN_ALLOW_THRESHOLD)),
            ScreenVerdict::Allow,
        ),
        (
            answer_with_allow_probability(&SCREEN_QUESTION, "allow", Some(0.96)),
            ScreenVerdict::Escalate,
        ),
        (
            answer_with_allow_probability(&SCREEN_QUESTION, "escalate", Some(1.0)),
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
    let verdicts: Vec<_> = options(&REVIEW_QUESTION)
        .iter()
        .map(|option| {
            let answers = answer(&REVIEW_QUESTION, option.id);
            (option.id, review_verdict(&answers).unwrap())
        })
        .collect();

    let expected: Vec<_> = options(&REVIEW_QUESTION)
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
    assert!(review_verdict(&[]).is_err());
}
