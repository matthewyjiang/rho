use std::fmt;

use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    Error, ProviderError, ProviderErrorKind, Retryability, Rho, RunEvent, SessionOptions,
    SystemPrompt, UserInput,
};
use serde_json::json;

const SECRET_CANARIES: [&str; 5] = [
    "RHO_AUDIT_API_KEY_7f3a",
    "RHO_AUDIT_OAUTH_ACCESS_1c9d",
    "RHO_AUDIT_REFRESH_442e",
    "RHO_AUDIT_COOKIE_88b1",
    "RHO_AUDIT_SIGNED_QUERY_629a",
];
const PROMPT_CANARY: &str = "RHO_AUDIT_PROMPT_CONTENT_d120";
const PROVIDER_CONTEXT_CANARY: &str = "RHO_AUDIT_PROVIDER_CONTEXT_901b";

#[derive(Clone)]
struct CanaryProvider {
    credentials: Vec<String>,
    fail: bool,
}

impl fmt::Debug for CanaryProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanaryProvider")
            .field("credentials", &"[REDACTED]")
            .field("fail", &self.fail)
            .finish()
    }
}

impl ModelProvider for CanaryProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("canary", "audit", "v1")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            if self.fail {
                Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "credential rejected by audit fixture",
                    Retryability::Permanent,
                ))
            } else {
                Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "safe completion".into(),
                )]))
            }
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            if self.fail {
                return self.send_turn(request).await;
            }
            events
                .send(ModelEvent::ProviderContext {
                    kind: "opaque_replay".into(),
                    position: Some(0),
                    data: json!({"opaque": PROVIDER_CONTEXT_CANARY}),
                })
                .await?;
            events
                .send(ModelEvent::OutputDelta("safe completion".into()))
                .await?;
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "safe completion".into(),
            )]))
        })
    }
}

fn assert_no_secret_canary(sink_name: &str, captured: &str) {
    for (index, canary) in SECRET_CANARIES.iter().enumerate() {
        assert!(
            !captured.contains(canary),
            "secret canary class {index} leaked into {sink_name}"
        );
        let encoded = canary.replace('_', "%5F");
        assert!(
            !captured.contains(&encoded),
            "encoded secret canary class {index} leaked into {sink_name}"
        );
    }
}

#[tokio::test]
async fn secret_canaries_are_absent_from_debug_events_diagnostics_snapshots_and_errors() {
    let provider = CanaryProvider {
        credentials: SECRET_CANARIES.iter().map(ToString::to_string).collect(),
        fail: false,
    };
    assert_eq!(provider.credentials.len(), SECRET_CANARIES.len());
    assert_no_secret_canary("provider Debug", &format!("{provider:?}"));

    let runtime = Rho::builder()
        .provider(provider)
        .system_prompt(SystemPrompt::Custom(PROMPT_CANARY.into()))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("audit")).await.unwrap();
    let mut event_debug = String::new();
    while let Some(event) = run.next_event().await {
        event_debug.push_str(&format!("{event:?}"));
    }
    run.outcome().await.unwrap();

    let diagnostics = format!("{:?}", session.diagnostics());
    let snapshot_json = session.snapshot().to_json().unwrap();
    let snapshot_debug = format!("{:?}", session.snapshot());
    assert_no_secret_canary("RunEvent Debug", &event_debug);
    assert_no_secret_canary("diagnostics", &diagnostics);
    assert_no_secret_canary("snapshot JSON", &snapshot_json);
    assert_no_secret_canary("snapshot Debug", &snapshot_debug);
    assert!(!diagnostics.contains(PROMPT_CANARY));
    assert!(snapshot_json.contains(PROMPT_CANARY));
    assert!(snapshot_json.contains(PROVIDER_CONTEXT_CANARY));
    assert!(!diagnostics.contains(PROVIDER_CONTEXT_CANARY));

    let failing = Rho::builder()
        .provider(CanaryProvider {
            credentials: SECRET_CANARIES.iter().map(ToString::to_string).collect(),
            fail: true,
        })
        .build()
        .unwrap();
    let failing_session = failing.session(SessionOptions::default()).await.unwrap();
    let mut failing_run = failing_session
        .start(UserInput::text("authentication failure"))
        .await
        .unwrap();
    let mut failed_event = None;
    while let Some(event) = failing_run.next_event().await {
        if matches!(event, RunEvent::Failed { .. }) {
            failed_event = Some(format!("{event:?}"));
        }
    }
    let error = failing_run.outcome().await.unwrap_err();
    assert!(matches!(error, Error::Provider(_)));
    assert_no_secret_canary("RunEvent::Failed", failed_event.as_deref().unwrap());
    assert_no_secret_canary("error Display", &error.to_string());
    assert_no_secret_canary("error Debug", &format!("{error:?}"));
}
