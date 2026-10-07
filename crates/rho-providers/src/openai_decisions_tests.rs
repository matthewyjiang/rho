use pretty_assertions::assert_eq;
use rho_sdk::{
    decision::{
        Answer, ChoiceAnswer, ChoiceOption, DecisionError, DecisionModel, DecisionRequest,
        NoulAnswer, NoulCriteria, Question, ScoreAnswer,
    },
    CancellationToken, SecretString,
};
use serde_json::json;

use super::OpenAiDecisionsModel;
use crate::decision_test_server::serve_once as server;

const VERDICT: Question<'static> = Question::choice(
    "verdict",
    "Pick one.",
    &[
        ChoiceOption::new("yes", "it does"),
        ChoiceOption::new("no", "it does not"),
    ],
);

const URGENT: Question<'static> = Question::noul(
    "urgent",
    "Is it urgent?",
    Some(NoulCriteria::new("needs action today", "can wait")),
);

const DONE: Question<'static> = Question::noul("done", "Is it done?", /*criteria*/ None);

const SEVERITY: Question<'static> =
    Question::score("severity", "How severe?", &["none", "minor", "major"]);

fn request<'a>(questions: &'a [Question<'a>]) -> DecisionRequest<'a> {
    DecisionRequest::new("Shared rules.", "the state", questions)
}

// Covers: the request reaches `{api_base}/decisions` in the API's shape for
// each question type (noul as `predicate`, with its criteria appended),
// carrying the shared instructions ahead of each question's and options and
// levels in declared order, from a base URL with or without a trailing slash,
// with the API key when one is set; each answer maps back to its question in
// question order whatever order the response lists them in.
// Owner: rho-providers OpenAI decisions client.
#[tokio::test]
async fn decide_sends_the_api_request_and_reads_each_answer_type() {
    for (trailing_slash, api_key) in [(false, None), (true, Some("sk-test"))] {
        let response = json!({
            "answers": [
                {
                    "type": "score", "name": "severity", "score": 1.5,
                    "probabilities": [
                        {"value": 2, "label": "2", "probability": 0.5},
                        {"value": 0, "label": "0", "probability": 0.0},
                        {"value": 1, "label": "1", "probability": 0.5},
                    ],
                    "confidence": 0.3,
                },
                {
                    "type": "choice", "name": "verdict", "choice": "no",
                    "probabilities": [
                        {"value": "yes", "probability": 0.25},
                        {"value": "no", "probability": 0.75},
                    ],
                    "confidence": 0.5,
                },
                {"type": "predicate", "name": "urgent", "probability": 0.9},
                {"type": "predicate", "name": "done", "probability": 0.1},
            ],
        });
        let (mut base, server) = server(200, response.to_string()).await;
        if trailing_slash {
            base.set_path("/v1/");
        }
        let model =
            OpenAiDecisionsModel::new(&base, "gpt-6-luna", api_key.map(SecretString::new)).unwrap();
        let questions = [VERDICT, URGENT, DONE, SEVERITY];

        let answers = model
            .decide(request(&questions), &CancellationToken::new())
            .await
            .unwrap();

        let received = server.await.unwrap();
        assert_eq!(
            (received.request_line.as_str(), received.authorization),
            (
                "POST /v1/decisions HTTP/1.1",
                api_key.map(|key| format!("Bearer {key}"))
            )
        );
        assert_eq!(
            received.body,
            json!({
                "model": "gpt-6-luna",
                "input": "the state",
                "questions": [
                    {
                        "type": "choice",
                        "name": "verdict",
                        "instructions": "Shared rules.\n\nPick one.",
                        "choices": [
                            {"value": "yes", "description": "it does"},
                            {"value": "no", "description": "it does not"},
                        ],
                    },
                    {
                        "type": "predicate",
                        "name": "urgent",
                        "instructions": "Shared rules.\n\nIs it urgent?\n\nTrue when: needs action today\nFalse when: can wait",
                    },
                    {
                        "type": "predicate",
                        "name": "done",
                        "instructions": "Shared rules.\n\nIs it done?",
                    },
                    {
                        "type": "score",
                        "name": "severity",
                        "instructions": "Shared rules.\n\nHow severe?",
                        "levels": [
                            {"label": "0", "description": "none"},
                            {"label": "1", "description": "minor"},
                            {"label": "2", "description": "major"},
                        ],
                    },
                ],
            })
        );
        assert_eq!(
            answers,
            vec![
                Answer::Choice(ChoiceAnswer::from_probabilities(1, vec![0.25, 0.75]).unwrap()),
                Answer::Noul(NoulAnswer::from_probability(0.9).unwrap()),
                Answer::Noul(NoulAnswer::from_probability(0.1).unwrap()),
                Answer::Score(ScoreAnswer::from_probabilities(vec![0.0, 0.5, 0.5]).unwrap()),
            ]
        );
    }
}

// Covers: a server error, a body that is not a decisions object, a missing,
// repeated, or unasked answer, an answer of another type, a choice outside the
// options or not the most likely one, probabilities that are not a
// distribution, and missing, repeated, extra, or mistyped probability values
// are errors, never a default or strengthened answer, and the error never
// repeats response text, where a server or proxy may echo the API key.
// Owner: rho-providers OpenAI decisions client.
#[tokio::test]
async fn unusable_responses_are_errors() {
    const KEY: &str = "sk-echoed";
    let choice = |choice: &str, probabilities: serde_json::Value| json!({"answers": [{"type": "choice", "name": "verdict", "choice": choice, "probabilities": probabilities}]});
    let distribution = |entries: &[(serde_json::Value, f64)]| {
        serde_json::Value::Array(
            entries
                .iter()
                .map(|(value, probability)| json!({"value": value, "probability": probability}))
                .collect(),
        )
    };
    let usable = distribution(&[(json!("yes"), 1.0), (json!("no"), 0.0)]);
    let cases = [
        (401, json!({"error": {"message": format!("bad key {KEY}")}})),
        (200, json!(format!("not an object {KEY}"))),
        (200, json!({"answers": []})),
        (
            200,
            json!({"answers": [{"type": "predicate", "name": "verdict", "probability": 0.9}]}),
        ),
        (
            200,
            choice(
                KEY,
                distribution(&[(json!("yes"), 0.5), (json!("no"), 0.5)]),
            ),
        ),
        (200, choice("no", usable.clone())),
        (
            200,
            choice(
                "yes",
                distribution(&[(json!("yes"), 1.5), (json!("no"), 0.0)]),
            ),
        ),
        (200, choice("yes", distribution(&[(json!("yes"), 1.0)]))),
        (
            200,
            choice(
                "yes",
                distribution(&[(json!("yes"), 0.9), (json!("no"), 0.0), (json!(KEY), 0.1)]),
            ),
        ),
        (
            200,
            choice(
                "yes",
                distribution(&[(json!("yes"), 0.5), (json!("yes"), 0.5), (json!("no"), 0.0)]),
            ),
        ),
        (
            200,
            choice("yes", distribution(&[(json!(0), 1.0), (json!(1), 0.0)])),
        ),
        (
            200,
            json!({"answers": [
                {"type": "choice", "name": "verdict", "choice": "yes", "probabilities": usable},
                {"type": "choice", "name": "verdict", "choice": "yes", "probabilities": usable},
            ]}),
        ),
        (
            200,
            json!({"answers": [
                {"type": "choice", "name": "verdict", "choice": "yes", "probabilities": usable},
                {"type": "predicate", "name": KEY, "probability": 0.5},
            ]}),
        ),
    ];
    for (status, response) in cases {
        let (base, server) = server(status, response.to_string()).await;
        let model =
            OpenAiDecisionsModel::new(&base, "gpt-6-luna", Some(SecretString::new(KEY))).unwrap();
        let questions = [VERDICT];

        let result = model
            .decide(request(&questions), &CancellationToken::new())
            .await;

        server.await.unwrap();
        let error = result.expect_err(&response.to_string());
        let message = format!("{:#}", anyhow::Error::from(error));
        assert!(!message.contains(KEY), "{message}");
    }

    let noul_and_score = [
        (
            URGENT,
            json!({"answers": [{"type": "predicate", "name": "urgent", "probability": 1.2}]}),
        ),
        (
            SEVERITY,
            json!({"answers": [{"type": "score", "name": "severity", "probabilities": [
                {"value": 0, "probability": 0.5}, {"value": 1, "probability": 0.5},
            ]}]}),
        ),
        (
            SEVERITY,
            json!({"answers": [{"type": "score", "name": "severity", "probabilities": [
                {"value": 0, "probability": 0.5}, {"value": 1, "probability": 0.5},
                {"value": 3, "probability": 0.0},
            ]}]}),
        ),
    ];
    for (question, response) in noul_and_score {
        let (base, server) = server(200, response.to_string()).await;
        let model = OpenAiDecisionsModel::new(&base, "gpt-6-luna", /*api_key*/ None).unwrap();
        let questions = [question];

        let result = model
            .decide(request(&questions), &CancellationToken::new())
            .await;

        server.await.unwrap();
        assert!(
            matches!(result, Err(DecisionError::InvalidResponse(_))),
            "{response}"
        );
    }
}
