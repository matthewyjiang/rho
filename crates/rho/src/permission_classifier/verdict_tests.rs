use pretty_assertions::assert_eq;

use super::{
    review_verdict, screen_verdict, ClassifierVerdict, ScreenVerdict, REVIEW_QUESTION,
    SCREEN_QUESTION,
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
// probabilities only at or above the allow percent, inclusive at both ends of
// its range; a text model's allow, which has none, still allows. Anything
// else escalates. Jev reports two decimals, so 0.95 must meet 95%.
// Owner: permission classifier screen answers.
#[test]
fn only_a_screen_allow_skips_review() {
    let allow =
        |probability| answer_with_allow_probability(&SCREEN_QUESTION, "allow", Some(probability));
    let cases = [
        (answer(&SCREEN_QUESTION, "allow"), 95, ScreenVerdict::Allow),
        (
            answer(&SCREEN_QUESTION, "escalate"),
            95,
            ScreenVerdict::Escalate,
        ),
        (Vec::new(), 95, ScreenVerdict::Escalate),
        (allow(0.95), 95, ScreenVerdict::Allow),
        (allow(0.949), 95, ScreenVerdict::Escalate),
        (allow(0.96), 97, ScreenVerdict::Escalate),
        (allow(0.5), 50, ScreenVerdict::Allow),
        (allow(1.0), 100, ScreenVerdict::Allow),
        (allow(0.999), 100, ScreenVerdict::Escalate),
        (
            answer_with_allow_probability(&SCREEN_QUESTION, "escalate", Some(0.4)),
            50,
            ScreenVerdict::Escalate,
        ),
    ];

    for (answers, allow_percent, expected) in cases {
        assert_eq!(
            screen_verdict(&answers, allow_percent),
            expected,
            "{allow_percent}% {answers:?}"
        );
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
