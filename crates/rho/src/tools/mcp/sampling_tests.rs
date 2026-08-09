use std::sync::Arc;

use pretty_assertions::assert_eq;
use serde_json::json;

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    HostInputResponse, SessionId,
};
use rmcp::model::CreateMessageRequestParams;

use super::{
    McpInFlightCalls, McpSamplingBridge, McpSamplingModel, McpSamplingPolicy, McpSamplingService,
};

const CONFIGURED_MODEL: &str = "rho-configured-model";

fn configured_provider(reply: &str) -> Arc<ScriptedProvider> {
    Arc::new(ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", CONFIGURED_MODEL),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text(reply.into()),
        ]))],
    ))
}

fn bound_bridge(provider: Arc<ScriptedProvider>) -> McpSamplingBridge {
    let bridge = McpSamplingBridge::new();
    bridge.bind(McpSamplingModel {
        provider,
        session_id: SessionId::new(),
        workspace_path: std::path::PathBuf::from("/"),
    });
    bridge
}

fn request(model_preferences: Option<serde_json::Value>) -> CreateMessageRequestParams {
    let mut params = json!({
        "messages": [{"role": "user", "content": {"type": "text", "text": "summarize this"}}],
        "maxTokens": 64,
        "systemPrompt": "You summarize.",
    });
    if let Some(preferences) = model_preferences {
        params["modelPreferences"] = preferences;
    }
    serde_json::from_value(params).expect("sampling request fixture parses")
}

/// Register one in-flight call and answer its single question with `allow`.
async fn with_answer<T>(
    calls: &McpInFlightCalls,
    allow: &'static str,
    work: impl std::future::Future<Output = T>,
) -> T {
    let (registration, mut questions) = calls.register();
    let (outcome, ()) = tokio::join!(work, async {
        let question = questions
            .recv()
            .await
            .expect("the confirmation reached the caller");
        let _ = question
            .reply
            .send(Ok(HostInputResponse::new().answer("allow", [allow])));
    });
    drop(registration);
    outcome
}

// Covers: a server that never opted into sampling must be refused before Rho
// reaches for a model, because config opt-in is the first of the two gates that
// stand between a server and the user's tokens.
// Owner: MCP sampling policy gate.
#[tokio::test]
async fn a_server_that_did_not_opt_in_is_rejected() {
    let provider = configured_provider("never asked");
    let calls = McpInFlightCalls::new();
    let (_registration, _questions) = calls.register();
    let service = McpSamplingService::new(
        "live",
        McpSamplingPolicy::Deny,
        bound_bridge(Arc::clone(&provider)),
        calls,
    );

    let error = service.create_message(request(None)).await.unwrap_err();

    assert_eq!(
        error.message,
        "this MCP server is not configured for sampling in Rho"
    );
    assert!(provider.recorded_requests().is_empty());
}

// Covers: an opted-in server still must not spend tokens on a request the user
// refused, because config opt-in alone would let a server sample in a loop.
// Owner: MCP sampling user gate.
#[tokio::test]
async fn a_refused_request_never_reaches_the_model() {
    let provider = configured_provider("never asked");
    let calls = McpInFlightCalls::new();
    let service = McpSamplingService::new(
        "live",
        McpSamplingPolicy::Ask,
        bound_bridge(Arc::clone(&provider)),
        calls.clone(),
    );

    let error = with_answer(&calls, "no", service.create_message(request(None)))
        .await
        .unwrap_err();

    assert_eq!(error.message, "the user refused this sampling request");
    assert!(provider.recorded_requests().is_empty());
}

// Covers: a server's `modelPreferences` must not select the model, because the
// model, provider, and credentials are the user's configuration and steering
// them would choose which of the user's accounts pays.
// Owner: MCP sampling request mapping.
#[tokio::test]
async fn model_preferences_do_not_change_the_model() {
    let provider = configured_provider("a summary");
    let calls = McpInFlightCalls::new();
    let service = McpSamplingService::new(
        "live",
        McpSamplingPolicy::Ask,
        bound_bridge(Arc::clone(&provider)),
        calls.clone(),
    );
    let preferences = json!({"hints": [{"name": "some-other-model"}], "costPriority": 0.0});

    let result = with_answer(
        &calls,
        "yes",
        service.create_message(request(Some(preferences))),
    )
    .await
    .unwrap();

    assert_eq!(result.model, CONFIGURED_MODEL);
    assert_eq!(
        result
            .message
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|text| text.text.clone()),
        Some("a summary".into())
    );
    let recorded = provider.recorded_requests();
    assert_eq!(recorded.len(), 1);
    // The server owns the system prompt; the conversation arrives as one user
    // turn because Rho's one-shot path takes exactly one.
    assert_eq!(
        recorded[0].messages,
        vec![
            rho_sdk::model::Message::System("You summarize.".into()),
            rho_sdk::model::Message::user_text("User: summarize this"),
        ]
    );
}

// Covers: an unbound model handle must fail the request rather than succeed
// quietly, because the binding is what proves a run has a model to spend.
// Owner: MCP sampling late binding.
#[tokio::test]
async fn an_unbound_model_fails_closed() {
    let calls = McpInFlightCalls::new();
    let (_registration, _questions) = calls.register();
    let bridge = McpSamplingBridge::new();
    let service = McpSamplingService::new("live", McpSamplingPolicy::Ask, bridge.clone(), calls);

    let unbound = service.create_message(request(None)).await.unwrap_err();
    assert_eq!(
        unbound.message,
        "Rho has no model bound for MCP sampling in this run"
    );

    // Binding and then releasing the model must return to the same refusal.
    bridge.bind(McpSamplingModel {
        provider: configured_provider("unused"),
        session_id: SessionId::new(),
        workspace_path: std::path::PathBuf::from("/"),
    });
    bridge.unbind();
    let after_unbind = service.create_message(request(None)).await.unwrap_err();
    assert_eq!(
        after_unbind.message,
        "Rho has no model bound for MCP sampling in this run"
    );
}

// Covers: a sampling request with no tool call to attribute it to must be
// rejected, because Rho would otherwise have no user to ask and no turn to
// charge.
// Owner: MCP sampling routing.
#[tokio::test]
async fn a_request_with_no_call_in_flight_is_rejected() {
    let provider = configured_provider("never asked");
    let service = McpSamplingService::new(
        "live",
        McpSamplingPolicy::Ask,
        bound_bridge(Arc::clone(&provider)),
        McpInFlightCalls::new(),
    );

    let error = service.create_message(request(None)).await.unwrap_err();

    assert_eq!(
        error.message,
        "Rho has no MCP tool call in flight to attribute this request to"
    );
    assert!(provider.recorded_requests().is_empty());
}
