use pretty_assertions::assert_eq;

use super::{
    Answer, ChoiceAnswer, ChoiceOption, DecisionError, DecisionRequest, InvalidRequest, NoulAnswer,
    Question, ScoreAnswer,
};

const A: ChoiceOption<'static> = ChoiceOption::new("a", "");
const B: ChoiceOption<'static> = ChoiceOption::new("b", "");
const PICK: Question<'static> = Question::choice("pick", "Pick one.", &[A, B]);

// Covers: a question or request some published System One server would
// reject fails the check, naming the rule it breaks.
// Owner: SDK decision request checks.
#[test]
fn requests_outside_the_shared_server_limits_fail_the_check() {
    const TOO_MANY_OPTIONS: [ChoiceOption<'static>; 27] =
        [const { ChoiceOption::new("x", "") }; 27];
    const SPACED: ChoiceOption<'static> = ChoiceOption::new("b c", "");
    const ELEVEN_LEVELS: [&str; 11] = [""; 11];
    let long_id = "x".repeat(101);
    let too_many_questions: Vec<Question<'_>> = (0..65)
        .map(|_| Question::noul("q", "?", /*criteria*/ None))
        .collect();
    let question_cases = [
        ("noul", Question::noul("ok", "?", None), Ok(())),
        ("choice", PICK, Ok(())),
        ("score", Question::score("s", "?", &["low", "high"]), Ok(())),
        (
            "one option",
            Question::choice("c", "?", &[A]),
            Err(InvalidRequest::OptionCount),
        ),
        (
            "27 options",
            Question::choice("c", "?", &TOO_MANY_OPTIONS),
            Err(InvalidRequest::OptionCount),
        ),
        (
            "duplicate option",
            Question::choice("c", "?", &[A, A]),
            Err(InvalidRequest::DuplicateOptionId),
        ),
        (
            "space in option ID",
            Question::choice("c", "?", &[A, SPACED]),
            Err(InvalidRequest::OptionId),
        ),
        (
            "one level",
            Question::score("s", "?", &["only"]),
            Err(InvalidRequest::LevelCount),
        ),
        (
            "11 levels",
            Question::score("s", "?", &ELEVEN_LEVELS),
            Err(InvalidRequest::LevelCount),
        ),
        (
            "space in question ID",
            Question::noul("a b", "?", None),
            Err(InvalidRequest::QuestionId),
        ),
        (
            "empty question ID",
            Question::noul("", "?", None),
            Err(InvalidRequest::QuestionId),
        ),
        (
            "101-char question ID",
            Question::noul(&long_id, "?", None),
            Err(InvalidRequest::QuestionId),
        ),
    ];
    for (name, question, expected) in question_cases {
        assert_eq!(question.check(), expected, "{name}");
        let questions = [question];
        assert_eq!(
            DecisionRequest::new("", "", &questions).check(),
            expected,
            "{name} in a request"
        );
    }

    let request_cases: [(&str, &[Question<'_>], _); 3] = [
        ("no questions", &[], Err(InvalidRequest::QuestionCount)),
        (
            "65 questions",
            &too_many_questions[..65],
            Err(InvalidRequest::QuestionCount),
        ),
        (
            "duplicate question ID",
            &[PICK, PICK],
            Err(InvalidRequest::DuplicateQuestionId),
        ),
    ];
    for (name, questions, expected) in request_cases {
        assert_eq!(
            DecisionRequest::new("", "", questions).check(),
            expected,
            "{name}"
        );
    }
}

// Covers: an answer cannot carry a probability outside 0 to 1, a choice
// that is not a most likely option, or probabilities that do not total 1
// beyond 2-decimal rounding, so a broken model response can never become a stronger
// answer; a score is the probability-weighted level.
// Owner: SDK decision answers.
#[test]
fn answers_hold_only_valid_probabilities() {
    assert_eq!(
        NoulAnswer::from_probability(0.75).map(|answer| (answer.value(), answer.probability())),
        Some((true, Some(0.75)))
    );
    assert_eq!(
        NoulAnswer::from_probability(0.25).map(|answer| answer.value()),
        Some(false)
    );
    for probability in [-0.01, 1.01, f64::NAN] {
        assert_eq!(
            NoulAnswer::from_probability(probability),
            None,
            "{probability}"
        );
    }

    let choice_cases = [
        (1, vec![0.25, 0.75], true),
        (2, vec![0.25, 0.75], false),
        (0, vec![1.5, 0.0], false),
        (0, vec![0.99, -0.01], false),
        (0, vec![0.5, f64::NAN], false),
        (0, vec![1.0, 1.0], false),
        (0, vec![0.5, 0.3], false),
        (0, vec![0.33, 0.33, 0.33], true),
        (0, vec![0.25, 0.75], false),
        (0, vec![0.5, 0.5], true),
    ];
    for (option, probabilities, valid) in choice_cases {
        let answer = ChoiceAnswer::from_probabilities(option, probabilities.clone());
        assert_eq!(answer.is_some(), valid, "{option} {probabilities:?}");
    }

    let score = ScoreAnswer::from_probabilities(vec![0.0, 0.5, 0.5]).unwrap();
    assert_eq!(score.value(), 1.5);
    for probabilities in [vec![], vec![0.5, 1.5], vec![1.0, 1.0, 1.0]] {
        assert_eq!(
            ScoreAnswer::from_probabilities(probabilities.clone()),
            None,
            "{probabilities:?}"
        );
    }
}

// Covers: answers from a model the caller did not write are checked against
// the questions: count, kind, an option or level the question has, and one
// probability per option or level.
// Owner: SDK decision answers.
#[test]
fn answers_that_do_not_fit_the_request_fail_the_check() {
    const LEVEL: Question<'static> = Question::score("level", "?", &["low", "mid", "high"]);
    let questions = [PICK, LEVEL];
    let request = DecisionRequest::new("", "", &questions);
    let pick = |answer: ChoiceAnswer| Answer::Choice(answer);
    let level = |answer: ScoreAnswer| Answer::Score(answer);
    let fitting_level = level(ScoreAnswer::from_level(2));
    let cases = [
        (
            "fits",
            vec![pick(ChoiceAnswer::from_option(1)), fitting_level.clone()],
            true,
        ),
        (
            "fits with probabilities",
            vec![
                pick(ChoiceAnswer::from_probabilities(0, vec![0.9, 0.1]).unwrap()),
                level(ScoreAnswer::from_probabilities(vec![0.2, 0.3, 0.5]).unwrap()),
            ],
            true,
        ),
        (
            "too few answers",
            vec![pick(ChoiceAnswer::from_option(1))],
            false,
        ),
        (
            "wrong kind",
            vec![
                Answer::Noul(NoulAnswer::from_value(true)),
                fitting_level.clone(),
            ],
            false,
        ),
        (
            "option out of range",
            vec![pick(ChoiceAnswer::from_option(2)), fitting_level.clone()],
            false,
        ),
        (
            "one probability for two options",
            vec![
                pick(ChoiceAnswer::from_probabilities(0, vec![1.0]).unwrap()),
                fitting_level.clone(),
            ],
            false,
        ),
        (
            "weighted level past the top within the tolerance",
            vec![
                pick(ChoiceAnswer::from_option(1)),
                level(ScoreAnswer::from_probabilities(vec![0.0, 0.01, 1.0]).unwrap()),
            ],
            true,
        ),
        (
            "level out of range",
            vec![
                pick(ChoiceAnswer::from_option(1)),
                level(ScoreAnswer::from_level(3)),
            ],
            false,
        ),
        (
            "two probabilities for three levels",
            vec![
                pick(ChoiceAnswer::from_option(1)),
                level(ScoreAnswer::from_probabilities(vec![0.5, 0.5]).unwrap()),
            ],
            false,
        ),
    ];
    for (name, answers, fits) in cases {
        let result = request.check_answers(&answers);
        assert_eq!(result.is_ok(), fits, "{name}: {result:?}");
        if let Err(error) = result {
            assert!(
                matches!(error, DecisionError::InvalidResponse(_)),
                "{name}: {error:?}"
            );
        }
    }
}
