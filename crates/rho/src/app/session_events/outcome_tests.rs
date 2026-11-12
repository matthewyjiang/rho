use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;
use rho_protocol::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome};
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
    Rho, SessionOptions,
};
use serde_json::json;

use super::{diagnostic_notice, finished, turn_error, withheld_diagnostic};
use crate::config::Config;

/// A session config whose model runs on `provider` with `auth`.
fn model(provider: &str, auth: &str) -> Config {
    Config {
        provider: provider.into(),
        auth: auth.into(),
        ..Config::default()
    }
}

/// The outcome of a scripted run whose one model step is `response`,
/// limited to `max_steps` steps when given.
async fn finished_run(response: ModelResponse, max_steps: Option<NonZeroUsize>) -> TurnOutcome {
    let mut builder = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "outcome"),
            [ScriptedTurn::completed(response)],
        ))
        .tool(ScriptedTool::new(
            ToolSpec {
                name: "noop".into(),
                description: "no-op".into(),
                input_schema: json!({"type": "object"}),
            },
            ScriptedToolOutcome::Success(ToolOutput::text("ok")),
        ));
    if let Some(max_steps) = max_steps {
        builder = builder.max_steps(max_steps);
    }
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let outcome = session.complete("go").await.unwrap();
    runtime.shutdown();
    finished(&outcome)
}

// Covers: a run that reached the model's answer is answered with the text
// whole and why the model stopped; one that used its step limit first is
// not an answer.
// Owner: turn outcome on the wire.
#[tokio::test]
async fn a_finished_run_is_answered_only_when_the_model_answered() {
    let answer = ModelResponse::Assistant(vec![ContentBlock::Text("the answer".into())]);
    let tool_call = ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall::new(
        "call-1".into(),
        "noop".into(),
        json!({}),
    ))]);
    let cases = [
        (
            "answered",
            answer,
            None,
            TurnOutcome::Answered {
                text: "the answer".into(),
                stop: AnswerStop::EndTurn,
            },
        ),
        (
            "step limit",
            tool_call,
            NonZeroUsize::new(1),
            TurnOutcome::MaxSteps {
                text: String::new(),
            },
        ),
    ];
    for (case, response, max_steps, expected) in cases {
        assert_eq!(finished_run(response, max_steps).await, expected, "{case}");
    }
}

// Covers: a failure's kind comes from the typed errors in its chain, never
// its text; its message says each cause once (dropping only a cause its
// error's message ends with), except an authentication
// failure's, which names only the provider and auth and how to log in.
// Owner: turn outcome on the wire.
#[test]
fn a_failure_is_classified_by_type_and_hides_authentication_text() {
    let provider = |kind| {
        anyhow::Error::new(rho_sdk::Error::Provider(rho_sdk::ProviderError::new(
            kind,
            "token fixture-secret rejected",
            rho_sdk::Retryability::Permanent,
        )))
    };
    let tool = anyhow::Error::new(rho_sdk::Error::Tool(rho_sdk::tool::ToolError::new(
        rho_sdk::tool::ToolErrorKind::Execution,
        "host failed",
    )))
    .context("the turn's tools stopped");
    let cases = [
        (
            provider(rho_sdk::ProviderErrorKind::Authentication),
            TurnErrorKind::Authentication,
            "authentication failed for xai (auth: xai-oauth); run /login xai or pass --auth xai-api-key",
        ),
        (
            provider(rho_sdk::ProviderErrorKind::Other),
            TurnErrorKind::Provider,
            "xai: provider failed: token fixture-secret rejected",
        ),
        (
            tool,
            TurnErrorKind::ToolHost,
            "the turn's tools stopped: tool failed: host failed",
        ),
        (
            anyhow::anyhow!("provider unavailable"),
            TurnErrorKind::Other,
            "provider unavailable",
        ),
        (
            anyhow::anyhow!("disk full").context("write failed: disk full at /tmp"),
            TurnErrorKind::Other,
            "write failed: disk full at /tmp: disk full",
        ),
    ];
    for (error, kind, message) in cases {
        assert_eq!(
            turn_error(&error, &model("xai", "xai-oauth")),
            TurnError {
                kind,
                message: message.into(),
            },
            "{error:#}"
        );
    }
}

// Covers: a failed turn shows its provider's diagnostic, unless the turn
// failed to authenticate, whose diagnostic may echo a credential: that one
// is withheld for the host's log, and no other is.
// Owner: turn outcome on the wire.
#[test]
fn a_diagnostic_shows_unless_authentication_failed() {
    let provider = |kind, diagnostic: Option<&str>| {
        let error =
            rho_sdk::ProviderError::new(kind, "provider failed", rho_sdk::Retryability::Permanent);
        let error = match diagnostic {
            Some(diagnostic) => error.with_diagnostic(diagnostic),
            None => error,
        };
        anyhow::Error::new(rho_sdk::Error::Provider(error)).context("the turn failed")
    };
    let cases = [
        (
            "provider failure",
            provider(rho_sdk::ProviderErrorKind::Other, Some("upstream said 500")),
            Some("provider diagnostic:\nupstream said 500"),
            None,
        ),
        (
            "authentication failure",
            provider(
                rho_sdk::ProviderErrorKind::Authentication,
                Some("token fixture-secret rejected"),
            ),
            None,
            Some("token fixture-secret rejected"),
        ),
        (
            "no diagnostic",
            provider(rho_sdk::ProviderErrorKind::Other, None),
            None,
            None,
        ),
        (
            "authentication failure without a diagnostic",
            provider(rho_sdk::ProviderErrorKind::Authentication, None),
            None,
            None,
        ),
    ];
    for (case, error, shown, withheld) in cases {
        assert_eq!(
            (
                diagnostic_notice(&error).as_deref(),
                withheld_diagnostic(&error)
            ),
            (shown, withheld),
            "{case}"
        );
    }
}

// Covers: an authentication failure's message offers `--auth` only for the
// provider's other keyed mode, if it has one.
// Owner: turn outcome on the wire.
#[test]
fn an_authentication_failure_names_the_way_to_log_in() {
    let error = anyhow::Error::new(rho_sdk::Error::Provider(rho_sdk::ProviderError::new(
        rho_sdk::ProviderErrorKind::Authentication,
        "token fixture-secret rejected",
        rho_sdk::Retryability::Permanent,
    )));
    for (provider, auth, expected) in [
        (
            "openai-codex",
            "codex",
            "authentication failed for openai-codex (auth: codex); run /login openai-codex",
        ),
        (
            "xai",
            "xai-api-key",
            "authentication failed for xai (auth: xai-api-key); run /login xai or pass --auth xai-oauth",
        ),
    ] {
        assert_eq!(
            turn_error(&error, &model(provider, auth)).message,
            expected,
            "{provider}/{auth}"
        );
    }
}
