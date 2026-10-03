use pretty_assertions::assert_eq;
use rho_sdk::CancellationToken;
use serde_json::json;

use super::{
    request_body, test_server::serve_once as server, SystemOneModel, MAX_BODY_BYTES,
    STATE_BUDGET_TOKENS,
};
use crate::decision::{ChoiceOption, ChoiceQuestion, DecisionModel, DecisionRequest};

const VERDICT: ChoiceQuestion = ChoiceQuestion {
    id: "verdict",
    instructions: "Pick one.",
    options: &[
        ChoiceOption {
            id: "yes",
            description: "it does",
        },
        ChoiceOption {
            id: "no",
            description: "it does not",
        },
    ],
};

fn request<'a>(questions: &'a [ChoiceQuestion]) -> DecisionRequest<'a> {
    DecisionRequest {
        instructions: "Shared rules.",
        state: "the state",
        questions,
    }
}

// Covers: the request reaches `{host}/v1/systemone` in the API's shape, with
// the shared instructions ahead of each question's and options as criteria in
// declared order, from a base URL with or without a trailing slash, carrying
// the API key when one is set; the answer maps back to the declared option
// with its probabilities.
// Owner: decision protocol System One backend.
#[tokio::test]
async fn decide_sends_the_api_request_and_reads_the_choice() {
    for (trailing_slash, api_key) in [(false, None), (true, Some("secret"))] {
        let response = json!({
            "model": "clef-flash",
            "answers": {"verdict": {
                "type": "choice",
                "choice": "no",
                "probabilities": {"yes": 0.25, "no": 0.75},
                "confidence": 0.5,
            }},
            "usage": {"input_tokens": 10, "output_tokens": 0},
        });
        let (mut base, server) = server(200, response.to_string()).await;
        if trailing_slash {
            base.set_path("/v1/");
        }
        let model =
            SystemOneModel::new(&base, "clef-flash".into(), api_key.map(str::to_owned)).unwrap();
        let questions = [VERDICT];

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
        let body = received.body;
        assert_eq!(
            body,
            json!({
                "model": "clef-flash",
                "state": "the state",
                "questions": {"verdict": {
                    "type": "choice",
                    "instructions": "Shared rules.\n\nPick one.",
                    "criteria": {"yes": "it does", "no": "it does not"},
                }},
            })
        );
        assert_eq!(
            serde_json::to_string(&body["questions"]["verdict"]["criteria"]).unwrap(),
            r#"{"yes":"it does","no":"it does not"}"#,
            "criteria keep declared order"
        );
        let answer = &answers["verdict"];
        assert_eq!(
            (
                answer.option.id,
                answer.probability("yes"),
                answer.probability("no")
            ),
            ("no", Some(0.25), Some(0.75))
        );
    }
}

// Covers: a server error, an answer that does not name an asked question and
// one of its options, or a probability outside 0 to 1 is an error, never a
// default or strengthened answer, and the error never repeats response text,
// where a server or proxy may echo the API key.
// Owner: decision protocol System One backend.
#[tokio::test]
async fn unusable_responses_are_errors() {
    const KEY: &str = "sk-echoed";
    let cases = [
        (
            401,
            json!({"error": format!("invalid key Bearer {KEY}")}).to_string(),
        ),
        (200, format!("not json {KEY}")),
        (200, json!({"answers": {}}).to_string()),
        (200, json!({"answers": {"verdict": KEY}}).to_string()),
        (
            200,
            json!({"answers": {"verdict": {"choice": KEY, "probabilities": {}}}}).to_string(),
        ),
        (
            200,
            json!({"answers": {"verdict": {"choice": "yes", "probabilities": {"yes": 1.5, "no": 0.0}}}})
                .to_string(),
        ),
        (
            200,
            json!({"answers": {"verdict": {"choice": "yes", "probabilities": {"yes": 0.99, KEY: -0.01}}}})
                .to_string(),
        ),
    ];
    for (status, response) in cases {
        let (base, server) = server(status, response.clone()).await;
        let model = SystemOneModel::new(&base, "clef-flash".into(), Some(KEY.into())).unwrap();
        let questions = [VERDICT];

        let result = model
            .decide(request(&questions), &CancellationToken::new())
            .await;

        server.await.unwrap();
        let error = format!("{:#}", result.expect_err(&response));
        assert!(!error.contains(KEY), "{error}");
    }
}

// Covers: a state over the model's budget fails before sending, naming the
// estimate and the budget, instead of the server's opaque context error.
// Owner: decision protocol System One backend.
#[tokio::test]
async fn oversized_state_names_its_estimate_and_the_budget() {
    // Never contacted: the request fails before sending.
    let base = url::Url::parse("http://127.0.0.1:9/v1").unwrap();
    let model = SystemOneModel::new(&base, "clef".into(), /*api_key*/ None).unwrap();
    let state = "x".repeat(usize::try_from(STATE_BUDGET_TOKENS).unwrap() * 4 + 4);
    let questions = [VERDICT];
    let oversized = DecisionRequest {
        state: &state,
        ..request(&questions)
    };

    let error = model
        .decide(oversized, &CancellationToken::new())
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "decision state needs ~{} tokens; the System One state budget is {STATE_BUDGET_TOKENS}",
            STATE_BUDGET_TOKENS + 1
        )
    );
}

// Covers: a request over the server's body limit fails before sending, with
// the limit and the asked size, instead of an opaque 413.
// Owner: decision protocol System One backend.
#[test]
fn oversized_request_names_its_size_and_the_limit() {
    let questions = [VERDICT];
    let fits = request_body("m", request(&questions)).unwrap().len();
    let state = "x".repeat(MAX_BODY_BYTES - fits + "the state".len() + 1);
    let oversized = DecisionRequest {
        state: &state,
        ..request(&questions)
    };

    let error = request_body("m", oversized).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "decision request body is {} bytes; the System One limit is {MAX_BODY_BYTES} bytes",
            MAX_BODY_BYTES + 1
        )
    );
}
