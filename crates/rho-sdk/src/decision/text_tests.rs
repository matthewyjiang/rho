use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::{parse_answers, questions_block, system_prompt, AnswerStyle, TextDecisionModel};
use crate::{
    decision::{
        Answer, ChoiceAnswer, ChoiceOption, DecisionError, DecisionModel, DecisionRequest,
        NoulAnswer, NoulCriteria, Question, QuestionKind, ScoreAnswer,
    },
    model::{ContentBlock, Message, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    CancellationToken,
};

const COLOR: Question<'static> = Question::choice(
    "color",
    "Pick a color.",
    &[
        ChoiceOption::new("red", "red"),
        ChoiceOption::new("blue", "blue"),
    ],
);

const SIZE: Question<'static> = Question::choice(
    "size",
    "Pick a size.",
    &[
        ChoiceOption::new("small", "small"),
        ChoiceOption::new("large", "large"),
    ],
);

const URGENT: Question<'static> = Question::noul(
    "urgent",
    "Is it urgent?",
    Some(NoulCriteria::new("needs action today", "can wait")),
);

const SEVERITY: Question<'static> =
    Question::score("severity", "How severe is it?", &["none", "minor", "major"]);

/// The chosen option's ID, for a choice answer to `question`.
fn option_id(answer: &Answer, question: &Question<'static>) -> &'static str {
    let (Answer::Choice(answer), QuestionKind::Choice(options)) = (answer, question.kind) else {
        panic!("not a choice answer: {answer:?}");
    };
    options[answer.option()].id
}

// Covers: the answer is exactly the whole direct response or the final line
// of a reasoned one, never text before it. A reasoned response with a code
// fence or a line break other than LF or CRLF anywhere, which could make its
// last line quoted text, is not read. The answer must name exactly the questions asked, each
// once (escaped or not), with an option ID.
// Owner: SDK decision text-model adapter.
#[test]
fn answers_are_exactly_the_final_answer_line() {
    use AnswerStyle::{Direct, Reasoned};
    let questions = [COLOR, SIZE];
    let cases = [
        (Direct, r#" {"color":"red","size":"large"} "#, Some(["red", "large"])),
        (Direct, "```json\n{\"color\":\"red\",\"size\":\"large\"}\n```", None),
        (Direct, r#"Sure: {"color":"red","size":"large"}"#, None),
        (
            Direct,
            "{\"color\":\"blue\",\"size\":\"small\"}\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "Weighing {color: \"}\" first.\r\n{\"color\":\"blue\",\"size\":\"small\"}\r\n",
            Some(["blue", "small"]),
        ),
        (
            Reasoned,
            "```text\n{\"color\":\"red\",\"size\":\"large\"}\n```\n{\"color\":\"blue\",\"size\":\"small\"}",
            None,
        ),
        (
            Reasoned,
            "````text\n```\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "~~~json\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "Reasoning.\n{\"color\":\"red\",\"size\":\"large\"}\n \t\n",
            Some(["red", "large"]),
        ),
        (
            Reasoned,
            "Run `a ``` b` first.\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            r#"{"color":"red","size":"large","\u0063olor":"blue"}"#,
            None,
        ),
        (
            Reasoned,
            "Quoted payload:\n```json\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "Reasoning.\n```json\n{\"color\":\"blue\",\"size\":\"small\"}\n```",
            None,
        ),
        (
            Reasoned,
            r#"The answer is {"color":"red","size":"small"}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"blue","size":"large","why":{"color":"red","size":"small"}}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"blue","color":"red","size":"large"}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"red","size":"large","color":"blue"}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"red","size":"large","shape":"round"}"#,
            None,
        ),
        (Reasoned, r#"{"color":"Red","size":"large"}"#, None),
        (Reasoned, r#"{"color":"green","size":"large"}"#, None),
        (Reasoned, r#"{"color":"red"}"#, None),
        (Reasoned, r#"{"color":["red"],"size":"large"}"#, None),
    ];

    let answer = r#"{"color":"red","size":"large"}"#;
    let separated = ["\r", "\u{0B}", "\u{0C}", "\u{85}", "\u{2028}", "\u{2029}"]
        .into_iter()
        .flat_map(|separator| {
            [
                format!("Reasoning.{separator}x\n{answer}"),
                format!("{separator}Reasoning.\n{answer}"),
                format!("Reasoning.\n{answer}{separator}"),
            ]
        });
    for text in separated {
        assert!(
            parse_answers(&text, &questions, Reasoned).is_err(),
            "{text:?}"
        );
    }

    for (style, text, expected) in cases {
        let actual = parse_answers(text, &questions, style).ok().map(|answers| {
            [
                option_id(&answers[0], &COLOR),
                option_id(&answers[1], &SIZE),
            ]
        });
        assert_eq!(actual, expected, "{style:?} {text}");
    }
}

// Covers: a parse error never repeats response text, which may hold secrets
// from the state.
// Owner: SDK decision text-model adapter.
#[test]
fn parse_errors_do_not_repeat_the_response() {
    const SECRET: &str = "sk-echoed";
    let questions = [COLOR, SIZE];
    let responses = [
        format!("\"{SECRET}\""),
        format!(r#"{{"{SECRET}":"red","{SECRET}":"red"}}"#),
        format!(r#"{{"color":"{SECRET}","size":"large"}}"#),
        format!(r#"{{"color":"red","{SECRET}":"large"}}"#),
    ];
    for response in responses {
        let error = parse_answers(&response, &questions, AnswerStyle::Direct).unwrap_err();
        let error = format!("{error:#}");
        assert!(!error.contains(SECRET), "{error}");
    }
}

// Covers: a noul question is answered `true` or `false` and a score
// question with a zero-based level, both without probabilities; any other
// answer ID is an error.
// Owner: SDK decision text-model adapter.
#[test]
fn noul_and_score_answers_are_read_from_their_answer_ids() {
    let questions = [URGENT, SEVERITY];
    let cases = [
        (
            r#"{"urgent":"true","severity":"2"}"#,
            Some(vec![
                Answer::Noul(NoulAnswer::from_value(true)),
                Answer::Score(ScoreAnswer::from_level(2)),
            ]),
        ),
        (
            r#"{"urgent":"false","severity":"0"}"#,
            Some(vec![
                Answer::Noul(NoulAnswer::from_value(false)),
                Answer::Score(ScoreAnswer::from_level(0)),
            ]),
        ),
        (r#"{"urgent":"yes","severity":"0"}"#, None),
        (r#"{"urgent":"true","severity":"3"}"#, None),
        (r#"{"urgent":"true","severity":"02"}"#, None),
        (r#"{"urgent":"true","severity":"none"}"#, None),
    ];
    for (text, expected) in cases {
        let actual = parse_answers(text, &questions, AnswerStyle::Direct).ok();
        assert_eq!(actual, expected, "{text}");
    }
}

// Covers: a noul question lists `true` and `false` with its criteria, and a
// score question lists its levels by zero-based index, lowest first.
// Owner: SDK decision text-model adapter.
#[test]
fn questions_block_lists_each_kind_by_answer_id() {
    let block = questions_block(&[URGENT, SEVERITY], AnswerStyle::Direct);

    let expected = "Answer every question below about the state.\n\
        \nQuestion `urgent`:\nIs it urgent?\n\
        Options:\n- `true`: needs action today\n- `false`: can wait\n\
        \nQuestion `severity`:\nHow severe is it?\n\
        Options, from lowest to highest:\n- `0`: none\n- `1`: minor\n- `2`: major\n\
        \nRespond with only a JSON object mapping each question ID to the ID of the \
        option you chose, with no code fence or other text: \
        {\"urgent\":\"<option ID>\",\"severity\":\"<option ID>\"}\n";
    assert_eq!(block, expected);
}

// Covers: the adapter sends the shared instructions as the system prompt and
// the state and questions as two user blocks, with no tools, and reads the
// answer; an invalid request fails before any provider call.
// Owner: SDK decision text-model adapter.
#[tokio::test]
async fn text_model_asks_one_turn_and_reads_the_answer() {
    let provider = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text(r#"{"color":"blue"}"#.into()),
        ]))],
    ));
    let model = TextDecisionModel::new(provider.clone(), AnswerStyle::Direct);
    let questions = [COLOR];
    let request = DecisionRequest::new("Shared rules.", "the state", &questions);

    let answers = model
        .decide(request, &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(answers, vec![Answer::Choice(ChoiceAnswer::from_option(1))]);
    let recorded = provider.recorded_requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        (&recorded[0].messages, recorded[0].tools.is_empty()),
        (
            &vec![
                Message::System(system_prompt("Shared rules.")),
                Message::User(vec![
                    ContentBlock::Text("the state".into()),
                    ContentBlock::Text(questions_block(&questions, AnswerStyle::Direct)),
                ]),
            ],
            true
        )
    );

    let duplicate = [COLOR, COLOR];
    let invalid = DecisionRequest::new("Shared rules.", "the state", &duplicate);
    let error = model
        .decide(invalid, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(error, DecisionError::InvalidRequest(_)),
        "{error:?}"
    );
    assert_eq!(provider.recorded_requests().len(), 1);
}
