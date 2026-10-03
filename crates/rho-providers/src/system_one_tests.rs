use pretty_assertions::assert_eq;
use rho_sdk::{
    decision::{
        Answer, ChoiceAnswer, ChoiceOption, DecisionError, DecisionModel, DecisionRequest,
        NoulAnswer, NoulCriteria, Question, ScoreAnswer,
    },
    CancellationToken, SecretString,
};
use serde_json::json;
use url::Url;

use super::{test_server::serve_once as server, SystemOneLimits, SystemOneModel};

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

// Covers: the request reaches `{api_base}/systemone` in the API's shape for
// each question type, with the shared instructions ahead of each question's
// and options and levels in declared order, from a base URL with or without
// a trailing slash, carrying the API key when one is set; each answer maps
// back to its question in question order, with its probabilities.
// Owner: rho-providers System One client.
#[tokio::test]
async fn decide_sends_the_api_request_and_reads_each_answer_type() {
    for (trailing_slash, api_key) in [(false, None), (true, Some("secret"))] {
        let response = json!({
            "model": "clef",
            "answers": {
                "severity": {
                    "type": "score", "score": 1.5,
                    "legend": {"0": "none", "1": "minor", "2": "major"},
                    "probabilities": {"0": 0.0, "1": 0.5, "2": 0.5}, "confidence": 0.3,
                },
                "verdict": {
                    "type": "choice", "choice": "no",
                    "probabilities": {"yes": 0.25, "no": 0.75}, "confidence": 0.5,
                },
                "urgent": {"type": "noul", "noul": 0.9},
                "done": {"type": "noul", "noul": 0.1},
            },
            "usage": {"input_tokens": 10, "output_tokens": 0},
        });
        let (mut base, server) = server(200, response.to_string()).await;
        if trailing_slash {
            base.set_path("/v1/");
        }
        let model = SystemOneModel::new(&base, "clef", api_key.map(SecretString::new)).unwrap();
        let questions = [VERDICT, URGENT, DONE, SEVERITY];

        let answers = model
            .decide(request(&questions), &CancellationToken::new())
            .await
            .unwrap();

        let received = server.await.unwrap();
        assert_eq!(
            (received.request_line.as_str(), received.authorization),
            (
                "POST /v1/systemone HTTP/1.1",
                api_key.map(|key| format!("Bearer {key}"))
            )
        );
        assert_eq!(
            received.body,
            json!({
                "model": "clef",
                "state": "the state",
                "questions": {
                    "verdict": {
                        "type": "choice",
                        "instructions": "Shared rules.\n\nPick one.",
                        "criteria": {"yes": "it does", "no": "it does not"},
                    },
                    "urgent": {
                        "type": "noul",
                        "instructions": "Shared rules.\n\nIs it urgent?",
                        "criteria": {"true": "needs action today", "false": "can wait"},
                    },
                    "done": {"type": "noul", "instructions": "Shared rules.\n\nIs it done?"},
                    "severity": {
                        "type": "score",
                        "instructions": "Shared rules.\n\nHow severe?",
                        "criteria": ["none", "minor", "major"],
                    },
                },
            })
        );
        assert!(
            received
                .raw_body
                .contains(r#""criteria":{"yes":"it does","no":"it does not"}"#),
            "criteria keep declared order: {}",
            received.raw_body
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

// Covers: a server error, an answer that is not an answer of its question's
// type to an asked question, an answer to a question not asked, a repeated
// key, probabilities that are not a distribution, a choice that is not the
// most likely option, or a missing or extra
// probability is an error, never a default or strengthened answer, and
// the error never repeats response text, where a server or proxy may echo
// the API key.
// Owner: rho-providers System One client.
#[tokio::test]
async fn unusable_responses_are_errors() {
    const KEY: &str = "sk-echoed";
    let choice = |choice: &str, probabilities: serde_json::Value| {
        json!({"answers": {"verdict": {"type": "choice", "choice": choice, "probabilities": probabilities}}})
            .to_string()
    };
    let cases = [
        (
            401,
            json!({"error": format!("invalid key Bearer {KEY}")}).to_string(),
        ),
        (200, format!("not json {KEY}")),
        (200, json!({"answers": {}}).to_string()),
        (200, json!({"answers": {"verdict": KEY}}).to_string()),
        (200, choice(KEY, json!({"yes": 0.5, "no": 0.5}))),
        (200, choice("yes", json!({"yes": 1.5, "no": 0.0}))),
        (200, choice("yes", json!({"yes": 0.99, "no": -0.01}))),
        (200, choice("yes", json!({"yes": 1.0}))),
        (200, choice("yes", json!({"yes": 0.9, "no": 0.0, KEY: 0.1}))),
        (
            200,
            json!({"answers": {"verdict": {"type": "noul", "noul": 0.9}}}).to_string(),
        ),
        (200, choice("yes", json!({"yes": 1.0, "no": 1.0}))),
        (200, choice("yes", json!({"yes": 0.25, "no": 0.75}))),
        (
            200,
            json!({"answers": {
                "verdict": {"type": "choice", "choice": "yes", "probabilities": {"yes": 1.0, "no": 0.0}},
                KEY: {"type": "noul", "noul": 0.5},
            }})
            .to_string(),
        ),
        // `json!` cannot repeat a key, so these are raw.
        (
            200,
            r#"{"answers":{"verdict":{"type":"choice","choice":"yes","probabilities":{"yes":-1,"\u0079es":0.99,"no":0.01}}}}"#
                .to_owned(),
        ),
        (
            200,
            r#"{"answers":{"verdict":{"type":"choice","choice":"no","probabilities":{"yes":0.0,"no":1.0}},"verdict":{"type":"choice","choice":"yes","probabilities":{"yes":1.0,"no":0.0}}}}"#
                .to_owned(),
        ),
    ];
    for (status, response) in cases {
        let (base, server) = server(status, response.clone()).await;
        let model = SystemOneModel::new(&base, "clef", Some(SecretString::new(KEY))).unwrap();
        let questions = [VERDICT];

        let result = model
            .decide(request(&questions), &CancellationToken::new())
            .await;

        server.await.unwrap();
        let error = format!("{:#}", anyhow::Error::from(result.expect_err(&response)));
        assert!(!error.contains(KEY), "{error}");
    }

    let noul_and_score = [
        json!({"answers": {"urgent": {"type": "noul", "noul": 1.2}}}),
        json!({"answers": {"severity": {"type": "score", "probabilities": {"0": 0.5, "1": 0.5}}}}),
        json!({"answers": {"severity": {"type": "score", "probabilities": {"0": 0.5, "1": 0.5, "3": 0.0}}}}),
    ];
    for response in noul_and_score {
        let (base, server) = server(200, response.to_string()).await;
        let model = SystemOneModel::new(&base, "clef", /*api_key*/ None).unwrap();
        let questions = [if response["answers"].get("urgent").is_some() {
            URGENT
        } else {
            SEVERITY
        }];

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

// Covers: a request over the model's body limit or state budget fails before
// sending, naming the asked size and the limit, instead of with the server's
// opaque error.
// Owner: rho-providers System One client.
#[tokio::test]
async fn requests_over_the_limits_fail_before_sending() {
    // Never contacted: these requests fail before sending.
    let base = Url::parse("http://127.0.0.1:9/v1").unwrap();
    let questions = [VERDICT];
    let state = "x".repeat(4 * 101);
    let oversized = DecisionRequest::new("Shared rules.", &state, &questions);
    let body_bytes = super::request_body("clef", oversized, /*max_bytes*/ None)
        .unwrap()
        .len();
    let cases = [
        (
            SystemOneLimits {
                state_budget: Some(100),
                ..SystemOneLimits::default()
            },
            "decision state needs ~101 tokens; the model's state budget is 100".to_owned(),
        ),
        (
            SystemOneLimits {
                max_body_bytes: Some(body_bytes - 1),
                ..SystemOneLimits::default()
            },
            format!(
                "decision request body is {body_bytes} bytes; the server limit is {} bytes",
                body_bytes - 1
            ),
        ),
    ];
    for (limits, expected) in cases {
        let model = SystemOneModel::new(&base, "clef", /*api_key*/ None)
            .unwrap()
            .with_limits(limits);

        let error = model
            .decide(oversized, &CancellationToken::new())
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), expected, "{limits:?}");
    }
}
