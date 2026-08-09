use pretty_assertions::assert_eq;
use serde_json::json;

use rho_sdk::HostInputResponse;
use rmcp::model::{ElicitRequestParams, ElicitationAction};

use super::{McpElicitationService, McpElicitationSupport, McpInFlightCalls};

fn form_request(properties: serde_json::Value, required: &[&str]) -> ElicitRequestParams {
    serde_json::from_value(json!({
        "message": "which colour?",
        "requestedSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        },
    }))
    .expect("elicitation request fixture parses")
}

fn colour_request() -> ElicitRequestParams {
    form_request(
        json!({"colour": {"type": "string", "enum": ["red", "blue"]}}),
        &["colour"],
    )
}

// Covers: an elicitation Rho cannot attribute to exactly one tool call must be
// declined rather than routed to a guess, because the wrong tool card would ask
// the user a question it did not cause.
// Owner: MCP elicitation routing.
#[tokio::test]
async fn unroutable_elicitations_are_declined() {
    let calls = McpInFlightCalls::new();
    let service =
        McpElicitationService::new("live", calls.clone(), McpElicitationSupport::Available);

    let with_no_call = service.elicit(colour_request()).await.unwrap();
    assert_eq!(with_no_call.action, ElicitationAction::Decline);
    assert_eq!(with_no_call.content, None);

    let (_first, _first_questions) = calls.register();
    let (_second, _second_questions) = calls.register();
    let with_two_calls = service.elicit(colour_request()).await.unwrap();
    assert_eq!(with_two_calls.action, ElicitationAction::Decline);
    assert_eq!(with_two_calls.content, None);
}

// Covers: a run that cannot show a questionnaire must decline even when a
// server asks anyway, because forwarding the question to a run with no
// questionnaire loop fails the whole run instead of one request.
// Owner: MCP elicitation routing.
#[tokio::test]
async fn a_run_that_cannot_ask_anyone_declines() {
    let calls = McpInFlightCalls::new();
    let (_registration, mut questions) = calls.register();
    let service = McpElicitationService::new("live", calls, McpElicitationSupport::Unavailable);

    let result = service.elicit(colour_request()).await.unwrap();

    assert_eq!(result.action, ElicitationAction::Decline);
    assert!(questions.try_recv().is_err(), "no question was raised");
}

// Covers: URL elicitation must be declined rather than accepted, because Rho
// opens no browser and an accept would tell the server the user had answered.
// Owner: MCP elicitation routing.
#[tokio::test]
async fn url_elicitation_is_declined() {
    let calls = McpInFlightCalls::new();
    let (_registration, _questions) = calls.register();
    let service = McpElicitationService::new("live", calls, McpElicitationSupport::Available);
    let request: ElicitRequestParams = serde_json::from_value(json!({
        "mode": "url",
        "message": "sign in",
        "url": "https://example.com/auth",
        "elicitationId": "one",
    }))
    .unwrap();

    let result = service.elicit(request).await.unwrap();

    assert_eq!(result.action, ElicitationAction::Decline);
}

// Covers: the three protocol actions must follow what happened to the user's
// form, because a server reads accept, decline, and cancel as three different
// instructions.
// Owner: MCP elicitation routing.
#[tokio::test]
async fn answering_the_form_produces_the_matching_action() {
    let calls = McpInFlightCalls::new();
    let service =
        McpElicitationService::new("live", calls.clone(), McpElicitationSupport::Available);

    // Accept: the user answered, so the typed content goes back.
    let (registration, mut questions) = calls.register();
    let (accepted, ()) = tokio::join!(service.elicit(colour_request()), async {
        let question = questions.recv().await.expect("the form reached the caller");
        assert_eq!(question.request.title(), "MCP server `live`: which colour?");
        let _ = question
            .reply
            .send(Ok(HostInputResponse::new().answer("colour", ["blue"])));
    });
    let accepted = accepted.unwrap();
    assert_eq!(accepted.action, ElicitationAction::Accept);
    assert_eq!(accepted.content, Some(json!({"colour": "blue"})));

    // Cancel: dismissing the form cancels the turn.
    let (cancelled, ()) = tokio::join!(service.elicit(colour_request()), async {
        let question = questions.recv().await.expect("the form reached the caller");
        let _ = question.reply.send(Err(rho_sdk::Error::Cancelled));
    });
    assert_eq!(cancelled.unwrap().action, ElicitationAction::Cancel);

    // Decline: the call ended, so there is nobody left to ask.
    drop(registration);
    let declined = service.elicit(colour_request()).await.unwrap();
    assert_eq!(declined.action, ElicitationAction::Decline);
}
