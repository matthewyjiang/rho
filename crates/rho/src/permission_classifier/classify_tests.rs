use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    decision::{
        Answer, ChoiceAnswer, DecisionError, DecisionFuture, DecisionModel, DecisionRequest,
        QuestionKind,
    },
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolResult},
    provider::{ScriptedProvider, ScriptedTurn},
    ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource, ProviderError,
    ProviderErrorKind, ProviderRequestUsageRecording, Retryability, SessionId,
};

use super::{
    check_screen_config,
    classify::{classify_capability_request_with_provider, ClassifyRequest, Screen},
    classify_capability_request, render_classifier_transcript, ClassifierVerdict, TranscriptBudget,
    TranscriptOverBudget, DECISION_SCREEN_ID, REVIEW_QUESTION,
};
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    config::{Config, InternalAgentModelConfig},
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
    let QuestionKind::Choice(options) = REVIEW_QUESTION.kind else {
        panic!("the review is a choice question");
    };
    let option = options
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
    run_pipeline_with_screener(provider, Screen::Classifier, reasoning, budget).await
}

async fn run_pipeline_with_screener(
    provider: &ScriptedProvider,
    screen: Screen,
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
        &screen,
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

/// A decision model that answers the screen once with `result` and records
/// the state it read.
struct FakeScreen {
    result: Mutex<Option<Result<Vec<Answer>, DecisionError>>>,
    state_budget: Option<u64>,
    /// Shared so a test can read it after the screen is boxed.
    states: Arc<Mutex<Vec<String>>>,
}

impl FakeScreen {
    fn new(result: Result<Vec<Answer>, DecisionError>, state_budget: Option<u64>) -> Self {
        Self {
            result: Mutex::new(Some(result)),
            state_budget,
            states: Arc::default(),
        }
    }

    /// A screen choosing `choice` (`0` allow, `1` escalate) with
    /// `allow_probability`.
    fn answering(choice: usize, allow_probability: f64) -> Self {
        let answer = ChoiceAnswer::from_probabilities(
            choice,
            vec![allow_probability, 1.0 - allow_probability],
        )
        .unwrap();
        Self::new(Ok(vec![Answer::Choice(answer)]), /*state_budget*/ None)
    }
}

impl DecisionModel for FakeScreen {
    fn decide<'a>(
        &'a self,
        request: DecisionRequest<'a>,
        _cancellation: &'a CancellationToken,
    ) -> DecisionFuture<'a> {
        self.states.lock().unwrap().push(request.state.to_owned());
        let result = self
            .result
            .lock()
            .unwrap()
            .take()
            .expect("screen asked twice");
        Box::pin(async move { result })
    }

    fn state_budget(&self) -> Option<u64> {
        self.state_budget
    }
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
            "confident allow",
            FakeScreen::answering(0, threshold),
            ClassifierVerdict::Allow,
            0,
        ),
        (
            "less confident allow",
            FakeScreen::answering(0, threshold - 0.01),
            deny("deny_scope_expansion"),
            1,
        ),
        (
            "escalate",
            FakeScreen::answering(1, 0.2),
            deny("deny_scope_expansion"),
            1,
        ),
        (
            "server error",
            FakeScreen::new(
                Err(DecisionError::Status { status: 500 }),
                /*state_budget*/ None,
            ),
            deny("deny_scope_expansion"),
            1,
        ),
    ];
    for (name, screen_model, expected, review_requests) in cases {
        let provider = ScriptedProvider::new(
            ModelIdentity::new("provider", "api", "model"),
            [text_turn(REVIEW_DENY)],
        );

        let (verdict, requests) = run_pipeline_with_screener(
            &provider,
            Screen::Decision(Box::new(screen_model)),
            ReasoningLevel::Medium,
            TranscriptBudget::Unbounded,
        )
        .await;

        assert_eq!(
            (verdict, requests.len()),
            (expected, review_requests),
            "{name}"
        );
    }
}

/// Where a screen under test recorded the state it read.
enum ScreenProbe {
    Decision(Arc<Mutex<Vec<String>>>),
    Text(Arc<ScriptedProvider>),
}

/// The transcript, the first user block, of a recorded text-model request.
fn request_state(request: &rho_sdk::provider::RecordedModelRequest) -> String {
    match request.messages.as_slice() {
        [_, Message::User(blocks)] => match blocks.first() {
            Some(ContentBlock::Text(text)) => text.clone(),
            other => panic!("unexpected block {other:?}"),
        },
        other => panic!("unexpected messages {other:?}"),
    }
}

// Covers: a screen with a smaller context than the review, a decision model
// or another text model, reads a transcript fitted to it, keeping the user's
// request and the pending call, while the review on the classifier's own
// model still reads every tool call that fits its budget.
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn screen_on_another_model_reads_a_transcript_fitted_to_its_budget() {
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
    let pending = pending_write();
    let screen_budget = TranscriptBudget::Tokens(10_500);
    let fitted = render_classifier_transcript(&history, &pending, screen_budget).unwrap();
    let full =
        render_classifier_transcript(&history, &pending, TranscriptBudget::Unbounded).unwrap();
    assert!(fitted.len() < full.len());

    let decision = FakeScreen::new(
        Ok(vec![Answer::Choice(ChoiceAnswer::from_option(1))]),
        Some(10_500),
    );
    let decision_states = decision.states.clone();
    let text = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("screen", "api", "small"),
        [text_turn(SCREEN_ESCALATE)],
    ));
    let cases = [
        (
            "decision",
            Screen::Decision(Box::new(decision)),
            ScreenProbe::Decision(decision_states),
        ),
        (
            "text",
            Screen::Text {
                provider: text.clone(),
                budget: screen_budget,
            },
            ScreenProbe::Text(text),
        ),
    ];
    for (name, screen, probe) in cases {
        let provider = ScriptedProvider::new(
            ModelIdentity::new("provider", "api", "model"),
            [text_turn(REVIEW_ALLOW)],
        );
        let session_id = SessionId::new();

        let verdict = classify_capability_request_with_provider(
            &provider,
            &screen,
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

        let screen_state = match probe {
            ScreenProbe::Decision(states) => states.lock().unwrap().concat(),
            ScreenProbe::Text(screen) => {
                let requests = screen.recorded_requests();
                assert_eq!(requests.len(), 1, "{name}: one screen request");
                request_state(&requests[0])
            }
        };
        let reviews = provider.recorded_requests();
        assert_eq!(reviews.len(), 1, "{name}: one review request");
        assert_eq!(
            (verdict, screen_state, request_state(&reviews[0])),
            (ClassifierVerdict::Allow, fitted.clone(), full.clone()),
            "{name}"
        );
    }
}

// Covers: a screen entry whose kind its provider cannot serve (a decision
// model off a decision host, a text model on a decision-only host), or whose
// auth mode the screen cannot send, fails the config check naming the
// configured value instead of being ignored, and a classification under it
// denies with that reason. A text model on a chat provider is valid; its
// credentials are covered by `has_credentials`.
// Owner: permission classifier model resolution
#[tokio::test]
async fn unusable_screen_config_is_reported() {
    use crate::config::ModelKind::{Decision, Text};
    let cases = [
        (None, Ok(())),
        (Some(("ollama", "clef", "none", None)), Ok(())),
        (Some(("ollama", "qwen3", "none", Some(Text))), Ok(())),
        (
            Some(("anthropic", "claude-haiku-4-5", "anthropic-api-key", Some(Decision))),
            Err(format!(
                "[internal_agents.{DECISION_SCREEN_ID}] kind `decision` needs a model on provider ollama or typesafe, got anthropic/claude-haiku-4-5"
            )),
        ),
        (
            Some(("typesafe", "jev-latest", "typesafe-api-key", Some(Text))),
            Err(format!(
                "[internal_agents.{DECISION_SCREEN_ID}] kind `text` needs a chat model, got typesafe/jev-latest"
            )),
        ),
        (
            Some(("ollama", "clef", "codex", None)),
            Err(format!(
                "[internal_agents.{DECISION_SCREEN_ID}] auth `codex` is not supported; use `none` or `ollama-api-key`"
            )),
        ),
    ];
    for (screen, expected) in cases {
        let mut config = Config::default();
        if let Some((provider, model, auth, kind)) = screen {
            let mut selection =
                InternalAgentModelConfig::new(provider.into(), model.into(), auth.into());
            selection.expect_rho_mut().kind = kind;
            config.set_internal_agent_model_config(DECISION_SCREEN_ID, selection);
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
