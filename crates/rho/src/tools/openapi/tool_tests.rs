use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::Tool,
    CapabilityOperation, CapabilityRequest, NetworkTarget, PolicyDecision, Rho, RunEvent,
    ScopedWorkspacePolicy, SessionOptions, ToolCompletion, UserInput, Workspace, WorkspacePolicy,
};
use serde_json::{json, Value};

use super::super::{
    response::MAX_RESPONSE_BYTES,
    test_server::{load_test_source, Recorded, Reply, TestServer},
};
use crate::tools::sdk_registry::ToolBundle as _;

/// `GET /items/{id}` and `GET /items?page&per_page`.
fn spec() -> Value {
    let ok = json!({"200": {"description": "ok", "content": {"application/json": {"schema": {}}}}});
    json!({
        "openapi": "3.1.0",
        "info": {"title": "test", "version": "1"},
        "paths": {
            "/items/{id}": {"get": {
                "operationId": "getItem",
                "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": ok,
            }},
            "/items": {"get": {
                "operationId": "listItems",
                "parameters": [
                    {"name": "page", "in": "query", "schema": {"type": "integer"}},
                    {"name": "per_page", "in": "query", "schema": {"type": "integer"}},
                ],
                "responses": ok,
            }},
        },
    })
}

/// The loaded tool for `operation_id`.
async fn tool(
    source: &str,
    env: &[(&str, &str)],
    max_response_bytes: usize,
    operation_id: &str,
) -> Arc<dyn Tool> {
    let outcome = load_test_source(&spec(), source, env, max_response_bytes).await;
    assert_eq!(outcome.report.errors, Vec::<String>::new());
    let bundle = outcome.bundle.expect("the source should load");
    let name = format!("openapi__test__{operation_id}");
    bundle
        .tools()
        .iter()
        .find(|tool| tool.spec().name == name)
        .cloned()
        .unwrap_or_else(|| panic!("no tool named {name}"))
}

#[derive(Clone, Default)]
struct RecordingPolicy {
    requests: Arc<Mutex<Vec<CapabilityRequest>>>,
}

impl WorkspacePolicy for RecordingPolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        self.requests.lock().unwrap().push(request.clone());
        ScopedWorkspacePolicy::new()
            .allow_network_host("127.0.0.1")
            .evaluate(request)
    }
}

impl RecordingPolicy {
    fn authorized_urls(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|request| match request.operation() {
                CapabilityOperation::NetworkAccess(NetworkTarget::Url(url)) => Some(url.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Run one model call of `tool` through a real runtime.
async fn call(tool: Arc<dyn Tool>, arguments: Value, policy: RecordingPolicy) -> ToolCompletion {
    let root = tempfile::tempdir().unwrap();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: tool.spec().name,
                    arguments,
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .workspace(Workspace::new(root.path()).unwrap())
        .workspace_policy(policy)
        .tool_shared(tool)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("call it")).await.unwrap();
    let mut completion = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished { result, .. } = event {
            completion = Some(result);
        }
    }
    run.outcome().await.unwrap();
    completion.expect("the tool should finish")
}

/// What reached a server: the target and the credential-bearing headers.
fn seen(requests: &[Recorded]) -> Vec<(String, Option<String>, Option<String>)> {
    requests
        .iter()
        .map(|request| {
            (
                request.target.clone(),
                request.headers.get("authorization").cloned(),
                request.headers.get("x-literal").cloned(),
            )
        })
        .collect()
}

// Covers: API keys, env headers, and literal headers go only to the base_url
// origin. A same-origin redirect keeps them; a cross-origin redirect sends
// none, and the URL the policy authorizes never holds the query key.
// Owner: OpenAPI tool transport (credential placement)
#[tokio::test]
async fn credentials_reach_only_the_source_origin() {
    let other = TestServer::start(|_| Reply::json(200, &json!({"from": "other"}))).await;
    let landing = other.url("/landing");
    let source = TestServer::start(move |request| match request.target.split('?').next() {
        Some("/items/same") => Reply::new(302, "").header("location", "/items/final"),
        Some("/items/away") => Reply::new(302, "").header("location", &landing),
        _ => Reply::json(200, &json!({"from": "source"})),
    })
    .await;
    let config = format!(
        r#"base_url = "{}"
headers = {{ x-literal = "literal" }}
headers_from_env = {{ authorization = "TOKEN" }}
auth = {{ type = "api_key", in = "query", name = "api_key", value_env = "KEY" }}"#,
        source.url("")
    );
    let env = [("TOKEN", "Bearer secret"), ("KEY", "k")];
    let get_item = tool(&config, &env, MAX_RESPONSE_BYTES, "getItem").await;

    let policy = RecordingPolicy::default();
    for id in ["same", "away"] {
        let completion = call(get_item.clone(), json!({"id": id}), policy.clone()).await;
        assert!(!completion.is_failure(), "{id}: {completion:?}");
    }

    let credentialed = |target: &str| {
        (
            target.to_owned(),
            Some("Bearer secret".to_owned()),
            Some("literal".to_owned()),
        )
    };
    assert_eq!(
        seen(&source.requests()),
        vec![
            credentialed("/items/same?api_key=k"),
            credentialed("/items/final?api_key=k"),
            credentialed("/items/away?api_key=k"),
        ]
    );
    assert_eq!(
        seen(&other.requests()),
        vec![("/landing".to_owned(), None, None)]
    );
    assert_eq!(
        policy.authorized_urls(),
        vec![source.url("/items/same"), source.url("/items/away")]
    );
}

// Covers: every HTTP answer reaches scripts as {status, body, next, headers};
// `next` holds the next call's arguments from the Link header, and a non-2xx
// answer fails the call for the model while keeping its data.
// Owner: OpenAPI tool result contract
#[tokio::test]
async fn results_carry_status_body_next_and_kept_headers() {
    let server = TestServer::start(|request| {
        if request.target.contains("page=9") {
            return Reply::json(404, &json!({"message": "Not Found"}));
        }
        Reply::json(200, &json!([1, 2]))
            .header(
                "link",
                "</items?page=2&per_page=50>; rel=\"next\", </items?page=5&per_page=50>; rel=\"last\"",
            )
            .header("x-ratelimit-remaining", "42")
            .header("x-unrelated", "dropped")
    })
    .await;
    let config = format!("base_url = \"{}\"", server.url(""));
    let list_items = tool(&config, &[], MAX_RESPONSE_BYTES, "listItems").await;

    let cases = [
        (
            json!({"page": 1, "per_page": 50}),
            false,
            json!({
                "status": 200,
                "body": [1, 2],
                "next": {"page": 2, "per_page": 50},
                "headers": {"x-ratelimit-remaining": "42"},
            }),
        ),
        (
            json!({"page": 9}),
            true,
            json!({
                "status": 404,
                "body": {"message": "Not Found"},
                "next": null,
                "headers": {},
            }),
        ),
    ];
    for (arguments, failed, data) in cases {
        let completion = call(list_items.clone(), arguments, RecordingPolicy::default()).await;
        let output = match completion {
            ToolCompletion::Success(output) | ToolCompletion::CompletedFailure(output) => output,
            other => panic!("expected a completed call, got {other:?}"),
        };
        assert_eq!(
            (output.is_failure(), output.structured_content()),
            (failed, Some(&data))
        );
    }
}

// Covers: a body over max_response_bytes fails the call naming the limit and
// the actual size, whether or not the server declares Content-Length.
// Owner: OpenAPI tool response cap
#[tokio::test]
async fn oversized_responses_fail_with_the_limit_and_the_size() {
    const LIMIT: usize = 1000;
    let server = TestServer::start(|request| {
        let body = vec![b'x'; 1500];
        match request.target.as_str() {
            "/items/chunked" => Reply::new(200, body).chunked(),
            _ => Reply::new(200, body),
        }
    })
    .await;
    let config = format!("base_url = \"{}\"", server.url(""));
    let get_item = tool(&config, &[], LIMIT, "getItem").await;

    for id in ["declared", "chunked"] {
        let completion = call(
            get_item.clone(),
            json!({"id": id}),
            RecordingPolicy::default(),
        )
        .await;
        let ToolCompletion::Failure(failure) = completion else {
            panic!("{id}: expected a failure, got {completion:?}");
        };
        assert_eq!(
            failure.message(),
            format!(
                "GET /items/{{id}} (test): response body is 1500 bytes, over max_response_bytes (1000 bytes)"
            ),
            "{id}"
        );
    }
}
