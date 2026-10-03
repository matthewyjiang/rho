use std::path::Path;

use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolResult},
    provider::{ScriptedProvider, ScriptedTurn},
    ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource, ProviderError,
    ProviderErrorKind, ProviderRequestUsageRecording, Retryability, SessionId,
};

use super::{
    check_screen_config,
    classify::{classify_capability_request_with_provider, ClassifyRequest},
    classify_capability_request, render_classifier_transcript, ClassifierVerdict, TranscriptBudget,
    TranscriptOverBudget, DECISION_SCREEN_ID, REVIEW_QUESTION,
};
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    config::{Config, InternalAgentModelConfig},
    decision::{
        system_one::{test_server::serve_once, SystemOneModel},
        DecisionModel,
    },
};

fn source(name: &str) -> CapabilitySource {
    CapabilitySource::built_in_tool(name)
}

fn sample_history() -> Vec<Message> {
    vec![
        Message::User(vec![ContentBlock::Text("please update config.toml".into())]),
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "write".into(),
            arguments: serde_json::json!({"path": "config.toml", "content": "x=1"}),
        })]),
    ]
}

fn pending_write() -> ApprovalRequest {
    ApprovalRequest::new(
        CapabilityRequest::write_path(
            "config.toml",
            rho_sdk::PathScope::PrimaryWorkspace,
            source("write"),
        ),
        "agent requested write access",
    )
}

fn text_turn(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

fn failed_turn() -> ScriptedTurn {
    ScriptedTurn::failed(ProviderError::new(
        ProviderErrorKind::Unavailable,
        "provider down",
        Retryability::Permanent,
    ))
}

fn deny(option_id: &str) -> ClassifierVerdict {
    let option = REVIEW_QUESTION
        .options
        .iter()
        .find(|option| option.id == option_id)
        .unwrap();
    ClassifierVerdict::Deny {
        reason: option.description.into(),
    }
}

const SCREEN_ALLOW: &str = r#"{"screen":"allow"}"#;
const SCREEN_ESCALATE: &str = r#"{"screen":"escalate"}"#;
const REVIEW_ALLOW: &str = r#"{"verdict":"allow"}"#;
const REVIEW_DENY: &str = r#"{"verdict":"deny_scope_expansion"}"#;

fn unavailable() -> ClassifierVerdict {
    ClassifierVerdict::Deny {
        reason: "classifier unavailable".into(),
    }
}

async fn run_pipeline(
    provider: &ScriptedProvider,
    reasoning: ReasoningLevel,
    budget: TranscriptBudget,
) -> (
    ClassifierVerdict,
    Vec<rho_sdk::provider::RecordedModelRequest>,
) {
    run_pipeline_with_screener(provider, /*screen_model*/ None, reasoning, budget).await
}

async fn run_pipeline_with_screener(
    provider: &ScriptedProvider,
    screen_model: Option<&dyn DecisionModel>,
    reasoning: ReasoningLevel,
    budget: TranscriptBudget,
) -> (
    ClassifierVerdict,
    Vec<rho_sdk::provider::RecordedModelRequest>,
) {
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();
    let verdict = classify_capability_request_with_provider(
        provider,
        screen_model,
        reasoning,
        budget,
        ClassifyRequest {
            history: &history,
            pending: &pending,
            cancellation: CancellationToken::new(),
            session_id: &session_id,
            workspace_path: Path::new("/test/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
        },
    )
    .await;
    (verdict, provider.recorded_requests())
}

// Covers: the screen decides whether the reasoned review runs, and only the review can deny
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn screen_result_decides_whether_review_runs() {
    let cases: Vec<(&str, Vec<ScriptedTurn>, ClassifierVerdict, usize)> = vec![
        (
            "screen allow skips review",
            vec![text_turn(SCREEN_ALLOW)],
            ClassifierVerdict::Allow,
            1,
        ),
        (
            "screen escalate reaches review allow",
            vec![text_turn(SCREEN_ESCALATE), text_turn(REVIEW_ALLOW)],
            ClassifierVerdict::Allow,
            2,
        ),
        (
            "screen escalate reaches review deny",
            vec![text_turn(SCREEN_ESCALATE), text_turn(REVIEW_DENY)],
            deny("deny_scope_expansion"),
            2,
        ),
        (
            "unreadable screen output still reaches review",
            vec![text_turn("allow"), text_turn(REVIEW_DENY)],
            deny("deny_scope_expansion"),
            2,
        ),
        (
            "screen provider error still reaches review",
            vec![failed_turn(), text_turn(REVIEW_ALLOW)],
            ClassifierVerdict::Allow,
            2,
        ),
        (
            "review reasoning before the answer still parses",
            vec![
                text_turn(SCREEN_ESCALATE),
                text_turn(&format!("The write is out of scope.\n{REVIEW_DENY}")),
            ],
            deny("deny_scope_expansion"),
            2,
        ),
        (
            "unparseable review fails closed",
            vec![
                text_turn(SCREEN_ESCALATE),
                text_turn(r#"{"decision":"allow"}"#),
            ],
            unavailable(),
            2,
        ),
        (
            "review provider error fails closed",
            vec![text_turn(SCREEN_ESCALATE), failed_turn()],
            unavailable(),
            2,
        ),
    ];

    for (name, turns, expected, expected_requests) in cases {
        let provider = ScriptedProvider::new(ModelIdentity::new("provider", "api", "model"), turns);
        let (verdict, requests) = run_pipeline(
            &provider,
            ReasoningLevel::Medium,
            TranscriptBudget::Unbounded,
        )
        .await;
        assert_eq!(verdict, expected, "{name}");
        assert_eq!(requests.len(), expected_requests, "{name}");
    }
}

fn screen_response(choice: &str, allow_probability: f64) -> String {
    serde_json::json!({"answers": {"screen": {
        "type": "choice",
        "choice": choice,
        "probabilities": {"allow": allow_probability, "escalate": 1.0 - allow_probability},
    }}})
    .to_string()
}

// Covers: a decision-model screen allows without a text-model call only on a
// confident allow; a less confident allow, an escalation, or a server error
// reaches the text-model review, which alone can deny.
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn decision_screen_skips_review_only_on_a_confident_allow() {
    let threshold = super::verdict::SCREEN_ALLOW_THRESHOLD;
    let cases = [
        (
            200,
            screen_response("allow", threshold),
            ClassifierVerdict::Allow,
            0,
        ),
        (
            200,
            screen_response("allow", threshold - 0.01),
            deny("deny_scope_expansion"),
            1,
        ),
        (
            200,
            screen_response("escalate", 0.2),
            deny("deny_scope_expansion"),
            1,
        ),
        (
            500,
            r#"{"error":"boom"}"#.to_owned(),
            deny("deny_scope_expansion"),
            1,
        ),
    ];
    for (status, response, expected, review_requests) in cases {
        let (base, server) = serve_once(status, response.clone()).await;
        let screen_model =
            SystemOneModel::new(&base, "clef-flash".into(), /*api_key*/ None).unwrap();
        let provider = ScriptedProvider::new(
            ModelIdentity::new("provider", "api", "model"),
            [text_turn(REVIEW_DENY)],
        );

        let (verdict, requests) = run_pipeline_with_screener(
            &provider,
            Some(&screen_model),
            ReasoningLevel::Medium,
            TranscriptBudget::Unbounded,
        )
        .await;

        server.await.unwrap();
        assert_eq!(
            (verdict, requests.len()),
            (expected, review_requests),
            "{response}"
        );
    }
}

// Covers: a decision-model screen gets a transcript fitted to its smaller
// context, keeping the user's request and the pending call, while the review
// still reads every tool call that fits its own budget.
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn decision_screen_reads_a_transcript_fitted_to_its_budget() {
    let mut history = sample_history();
    for index in 0..200 {
        history.push(Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: format!("old-{index}"),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "x".repeat(500)}),
        })]));
        history.push(Message::ToolResult(ToolResult {
            id: format!("old-{index}"),
            ok: true,
            content: "done".into(),
        }));
    }
    let (base, server) = serve_once(200, screen_response("escalate", 0.1)).await;
    let screen_model = SystemOneModel::new(&base, "clef".into(), /*api_key*/ None).unwrap();
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [text_turn(REVIEW_ALLOW)],
    );
    let pending = pending_write();
    let session_id = SessionId::new();

    let verdict = classify_capability_request_with_provider(
        &provider,
        Some(&screen_model),
        ReasoningLevel::Low,
        TranscriptBudget::Unbounded,
        ClassifyRequest {
            history: &history,
            pending: &pending,
            cancellation: CancellationToken::new(),
            session_id: &session_id,
            workspace_path: Path::new("/test/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
        },
    )
    .await;

    let screen_state = server.await.unwrap().body["state"]
        .as_str()
        .unwrap()
        .to_owned();
    let review_state = match provider.recorded_requests()[0].messages.as_slice() {
        [_, Message::User(blocks)] => match blocks.first() {
            Some(ContentBlock::Text(text)) => text.clone(),
            other => panic!("unexpected block {other:?}"),
        },
        other => panic!("unexpected messages {other:?}"),
    };
    let screen_budget = TranscriptBudget::Tokens(screen_model.state_budget().unwrap());
    let fitted = render_classifier_transcript(&history, &pending, screen_budget).unwrap();
    let full =
        render_classifier_transcript(&history, &pending, TranscriptBudget::Unbounded).unwrap();
    assert_eq!(verdict, ClassifierVerdict::Allow);
    assert!(fitted.len() < full.len());
    assert_eq!((screen_state, review_state), (fitted, full));
}

// Covers: a screen entry that is not an Ollama model, or uses an auth mode
// the screen cannot send, fails the config check naming the configured value
// instead of being ignored, and a classification under it denies with that
// reason.
// Owner: permission classifier model resolution
#[tokio::test]
async fn unusable_decision_screen_config_is_reported() {
    let cases = [
        (None, Ok(())),
        (Some(("ollama", "clef", "none")), Ok(())),
        (
            Some(("anthropic", "claude-haiku-4-5", "none")),
            Err(format!(
                "[internal_agents.{DECISION_SCREEN_ID}] must name a decision model on provider ollama, got anthropic/claude-haiku-4-5"
            )),
        ),
        (
            Some(("ollama", "clef", "codex")),
            Err(format!(
                "[internal_agents.{DECISION_SCREEN_ID}] auth `codex` is not supported; use `none` or `ollama-api-key`"
            )),
        ),
    ];
    for (screen, expected) in cases {
        let mut config = Config::default();
        if let Some((provider, model, auth)) = screen {
            config.set_internal_agent_model(
                DECISION_SCREEN_ID,
                provider.into(),
                model.into(),
                auth.into(),
            );
        }

        let result = check_screen_config(&config).map_err(|error| error.to_string());

        assert_eq!(result, expected, "{screen:?}");
        if let Err(reason) = expected {
            assert_eq!(
                verdict_for_config(config).await,
                ClassifierVerdict::Deny { reason }
            );
        }
    }
}

// Covers: the screen stays at Low while the review uses configured reasoning,
// and both stages share the system prompt and transcript block so the review
// can reuse the screen's cache prefix.
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn stages_share_the_cache_prefix_and_differ_in_reasoning() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [text_turn(SCREEN_ESCALATE), text_turn(REVIEW_ALLOW)],
    );
    let transcript = render_classifier_transcript(
        &sample_history(),
        &pending_write(),
        TranscriptBudget::Unbounded,
    )
    .unwrap();

    let (verdict, requests) =
        run_pipeline(&provider, ReasoningLevel::High, TranscriptBudget::Unbounded).await;

    assert_eq!(verdict, ClassifierVerdict::Allow);
    let [screen, review] = [&requests[0], &requests[1]].map(|request| {
        let [Message::System(system), Message::User(blocks)] = request.messages.as_slice() else {
            panic!("unexpected messages {:?}", request.messages);
        };
        let [ContentBlock::Text(state), ContentBlock::Text(questions)] = blocks.as_slice() else {
            panic!("unexpected blocks {blocks:?}");
        };
        (
            system.clone(),
            state.clone(),
            questions.clone(),
            request.reasoning_level,
            request.tools.is_empty(),
        )
    });
    assert_eq!(screen.0, review.0);
    assert_eq!((&screen.1, &review.1), (&transcript, &transcript));
    assert_ne!(screen.2, review.2);
    assert_eq!(
        (screen.3, screen.4, review.3, review.4),
        (ReasoningLevel::Low, true, ReasoningLevel::High, true)
    );
}

// Covers: a transcript that cannot fit the classifier context denies with the
// asked and allowed token counts, without sending an oversize request
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn over_budget_transcript_denies_visibly_without_a_model_call() {
    let provider = ScriptedProvider::new(ModelIdentity::new("provider", "api", "model"), []);

    let (verdict, requests) =
        run_pipeline(&provider, ReasoningLevel::Low, TranscriptBudget::Tokens(1)).await;

    let over_budget = render_classifier_transcript(
        &sample_history(),
        &pending_write(),
        TranscriptBudget::Tokens(1),
    )
    .unwrap_err()
    .downcast::<TranscriptOverBudget>()
    .unwrap();
    assert_eq!(over_budget.limit_tokens, 1);
    assert_eq!(
        verdict,
        ClassifierVerdict::Deny {
            reason: over_budget.to_string()
        }
    );
    assert!(requests.is_empty());
}

// Covers: an unset classifier model cannot fall back to the executor model
// Owner: permission classifier model resolution
#[tokio::test]
async fn missing_classifier_model_fails_closed() {
    assert_eq!(verdict_for_config(Config::default()).await, unavailable());
}

// Covers: the classifier never delegates to Claude runtime
// Owner: permission classifier model resolution
#[tokio::test]
async fn claude_runtime_selection_fails_closed() {
    let mut config = Config::default();
    config.set_internal_agent_model_config(
        PERMISSION_CLASSIFIER_AGENT_ID,
        InternalAgentModelConfig::claude_cli(None),
    );

    assert_eq!(verdict_for_config(config).await, unavailable());
}

async fn verdict_for_config(config: Config) -> ClassifierVerdict {
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();
    classify_capability_request(
        &config,
        ClassifyRequest {
            history: &history,
            pending: &pending,
            cancellation: CancellationToken::new(),
            session_id: &session_id,
            workspace_path: Path::new("/test/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
        },
    )
    .await
}
