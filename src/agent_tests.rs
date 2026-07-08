use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use super::*;
use crate::model::{ContextUsageSource, ModelProvider, ModelRequest, ModelResponse};
use crate::tool::{Tool, ToolCall, ToolSpec};

type RecordedRequests = Arc<Mutex<Vec<(String, Vec<Message>)>>>;

#[derive(Clone, Default)]
struct RecordingHistorySink {
    appended: Arc<Mutex<Vec<Message>>>,
    replaced: Arc<Mutex<Vec<Message>>>,
}

impl RecordingHistorySink {
    fn append_target(target: Arc<Mutex<Vec<Message>>>) -> Self {
        Self {
            appended: target,
            replaced: Arc::default(),
        }
    }

    fn replace_target(target: Arc<Mutex<Vec<Message>>>) -> Self {
        Self {
            appended: Arc::default(),
            replaced: target,
        }
    }
}

impl HistorySink for RecordingHistorySink {
    fn append_message(&mut self, message: &Message) -> anyhow::Result<()> {
        self.appended.lock().unwrap().push(message.clone());
        Ok(())
    }

    fn replace_history(&mut self, messages: &[Message]) -> anyhow::Result<()> {
        self.replaced
            .lock()
            .unwrap()
            .extend(messages.iter().cloned());
        Ok(())
    }
}

#[derive(Clone, Default)]
struct RecordingProvider {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    tools: Arc<Mutex<Vec<Vec<ToolSpec>>>>,
    prompt_cache_keys: Arc<Mutex<Vec<Option<String>>>>,
    response: Option<ModelResponse>,
}

#[async_trait(?Send)]
impl ModelProvider for RecordingProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.prompt_cache_keys
            .lock()
            .unwrap()
            .push(request.prompt_cache_key.clone());
        self.tools.lock().unwrap().push(request.tools.clone());
        self.requests.lock().unwrap().push(request.messages);
        Ok(self
            .response
            .clone()
            .unwrap_or_else(|| ModelResponse::Assistant(vec![ContentBlock::Text("ok".into())])))
    }
}

fn test_agent(provider: RecordingProvider) -> Agent {
    test_agent_with_tools(provider, ToolRegistry::new())
}

fn test_agent_with_tools(provider: impl ModelProvider + 'static, tools: ToolRegistry) -> Agent {
    Agent::new(
        Box::new(provider),
        tools,
        ToolContext {
            cwd: std::env::current_dir().unwrap(),
            max_output_bytes: 12000,
        },
    )
}

struct FailingProvider {
    requests: Arc<Mutex<usize>>,
    error: ModelError,
}

#[async_trait(?Send)]
impl ModelProvider for FailingProvider {
    async fn send_turn(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        *self.requests.lock().unwrap() += 1;
        Err(match &self.error {
            ModelError::MissingApiKey => ModelError::MissingApiKey,
            ModelError::InvalidResponse(message) => ModelError::InvalidResponse(message.clone()),
            _ => unreachable!("test only clones selected errors"),
        })
    }
}

struct TransientInvalidResponseProvider {
    requests: Arc<Mutex<usize>>,
}

#[async_trait(?Send)]
impl ModelProvider for TransientInvalidResponseProvider {
    async fn send_turn(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let mut requests = self.requests.lock().unwrap();
        *requests += 1;
        if *requests == 1 {
            return Err(ModelError::InvalidResponse(
                "temporary parse failure".into(),
            ));
        }
        Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
            "ok".into(),
        )]))
    }
}

struct SequencedProvider {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    responses: Mutex<VecDeque<ModelResponse>>,
}

#[async_trait(?Send)]
impl ModelProvider for SequencedProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.messages);
        Ok(self.responses.lock().unwrap().pop_front().unwrap())
    }
}

struct SequencedToolRecordingProvider {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    tools: Arc<Mutex<Vec<Vec<ToolSpec>>>>,
    responses: Mutex<VecDeque<ModelResponse>>,
}

#[async_trait(?Send)]
impl ModelProvider for SequencedToolRecordingProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.tools.lock().unwrap().push(request.tools.clone());
        self.requests.lock().unwrap().push(request.messages);
        Ok(self.responses.lock().unwrap().pop_front().unwrap())
    }
}

struct UsageStreamingProvider;

#[async_trait(?Send)]
impl ModelProvider for UsageStreamingProvider {
    async fn send_turn(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        unreachable!("streaming provider should use send_turn_stream")
    }

    async fn send_turn_stream(
        &self,
        _request: ModelRequest,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        on_event(ModelEvent::Usage(ModelUsage {
            input_tokens: Some(300),
            cache_read_tokens: Some(700),
            context_window: Some(10_000),
            ..ModelUsage::default()
        }))?;
        Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
            "ok".into(),
        )]))
    }
}

struct CompactingProvider {
    requests: RecordedRequests,
}

#[async_trait(?Send)]
impl ModelProvider for CompactingProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests
            .lock()
            .unwrap()
            .push(("summary".into(), request.messages));
        Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
            "compacted summary".into(),
        )]))
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let is_summary_request = matches!(
            request.messages.first(),
            Some(Message::System(text)) if text.starts_with("Summarize the conversation history")
        );
        if is_summary_request {
            self.requests
                .lock()
                .unwrap()
                .push(("summary".into(), request.messages));
            on_event(ModelEvent::Usage(ModelUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                ..ModelUsage::default()
            }))?;
            return Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "compacted summary".into(),
            )]));
        }
        self.requests
            .lock()
            .unwrap()
            .push(("normal".into(), request.messages));
        on_event(ModelEvent::Usage(ModelUsage {
            input_tokens: Some(900),
            ..ModelUsage::default()
        }))?;
        Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
            "ok".into(),
        )]))
    }
}

struct OkTool;

#[async_trait]
impl Tool for OkTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ok_tool".into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    async fn call(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            id,
            ok: true,
            content: "tool ok".into(),
        })
    }
}

struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "failing_tool".into(),
            description: "test failing tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    async fn call(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
        _id: String,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Message("tool failed".into()))
    }
}

#[tokio::test]
async fn does_not_retry_non_recoverable_provider_errors() {
    let requests = Arc::new(Mutex::new(0));
    let mut agent = Agent::new(
        Box::new(FailingProvider {
            requests: requests.clone(),
            error: ModelError::MissingApiKey,
        }),
        ToolRegistry::new(),
        ToolContext {
            cwd: std::env::current_dir().unwrap(),
            max_output_bytes: 12000,
        },
    );

    let err = agent.run("hello".into()).await.unwrap_err();

    assert!(matches!(
        err,
        AgentError::Provider(ModelError::MissingApiKey)
    ));
    assert_eq!(*requests.lock().unwrap(), 1);
}

#[tokio::test]
async fn retries_recoverable_invalid_response_errors() {
    let requests = Arc::new(Mutex::new(0));
    let mut agent = Agent::new(
        Box::new(TransientInvalidResponseProvider {
            requests: requests.clone(),
        }),
        ToolRegistry::new(),
        ToolContext {
            cwd: std::env::current_dir().unwrap(),
            max_output_bytes: 12000,
        },
    );

    let output = agent.run("hello".into()).await.unwrap();

    assert_eq!(output, "ok");
    assert_eq!(*requests.lock().unwrap(), 2);
}

#[test]
fn load_skill_truncates_contents_before_persisting() {
    let root = tempfile::tempdir().unwrap();
    let skill = crate::skills::Skill {
        name: "long-skill".into(),
        description: "long skill".into(),
        path: root.path().join(".agents/skills/long-skill/SKILL.md"),
        contents: "abcdefghijklmnopqrstuvwxyz".into(),
    };
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let mut agent = Agent::new(
        Box::new(RecordingProvider::default()),
        ToolRegistry::new(),
        ToolContext {
            cwd: root.path().to_path_buf(),
            max_output_bytes: 8,
        },
    );
    agent.set_history_sink(RecordingHistorySink::append_target(persisted.clone()));

    agent.load_skill(&skill).unwrap();

    let persisted = persisted.lock().unwrap();
    let Message::User(blocks) = persisted.last().unwrap() else {
        panic!("expected persisted user message");
    };
    let [ContentBlock::Text(text)] = blocks.as_slice() else {
        panic!("expected single text block");
    };
    assert!(text.contains("abcdefgh\n[truncated]"));
    assert!(!text.contains("ijklmnopqrstuvwxyz"));
}

#[tokio::test]
async fn includes_session_derived_prompt_cache_key_in_model_requests() {
    let provider = RecordingProvider::default();
    let prompt_cache_keys = provider.prompt_cache_keys.clone();
    let mut agent = test_agent(provider);

    agent.set_session_id(Some("session / one".into()));
    agent.run("first".into()).await.unwrap();
    agent.set_session_id(None);
    agent.run("second".into()).await.unwrap();

    assert_eq!(
        *prompt_cache_keys.lock().unwrap(),
        vec![Some("rho:session-one".into()), None]
    );
}

#[tokio::test]
async fn emits_estimated_context_usage_before_provider_call() {
    let mut agent = test_agent(RecordingProvider::default());
    let mut context_events = Vec::new();

    agent
        .run_with_events("hello".into(), |event| {
            if let AgentEvent::ContextUsage(usage) = event {
                context_events.push(usage);
            }
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(context_events.len(), 1);
    assert_eq!(context_events[0].source, ContextUsageSource::Estimated);
    assert!(context_events[0].tokens.unwrap() > 0);
}

#[tokio::test]
async fn emits_provider_reported_context_usage_from_model_usage() {
    let mut agent = test_agent_with_tools(UsageStreamingProvider, ToolRegistry::new());
    let mut context_events = Vec::new();

    agent
        .run_with_events("hello".into(), |event| {
            if let AgentEvent::ContextUsage(usage) = event {
                context_events.push(usage);
            }
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(context_events.len(), 2);
    assert_eq!(context_events[0].source, ContextUsageSource::Estimated);
    assert_eq!(
        context_events[1].source,
        ContextUsageSource::ProviderReported
    );
    assert_eq!(context_events[1].tokens, Some(1_000));
    assert_eq!(context_events[1].context_window, Some(10_000));
}

#[tokio::test]
async fn compacts_history_before_normal_provider_call_when_threshold_is_exceeded_with_configured_context_window(
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut agent = test_agent_with_tools(
        CompactingProvider {
            requests: requests.clone(),
        },
        ToolRegistry::new(),
    );
    agent.set_compaction_config(CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        recent_messages: 1,
    });
    agent.set_context_window(Some(1_000));

    agent.run("first".into()).await.unwrap();
    agent.run("second".into()).await.unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].0, "normal");
    assert_eq!(requests[1].0, "summary");
    assert_eq!(requests[2].0, "normal");
    assert!(matches!(
        requests[1].1.as_slice(),
        [Message::System(_), Message::User(_)]
    ));
    assert!(requests[2].1.iter().any(|message| {
        matches!(message, Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text.contains("compacted summary")))
    }));
    assert!(
        matches!(requests[2].1.last(), Some(Message::User(blocks)) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "second"))
    );
}

#[tokio::test]
async fn emits_usage_for_compaction_summary_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut agent = test_agent_with_tools(
        CompactingProvider {
            requests: requests.clone(),
        },
        ToolRegistry::new(),
    );
    agent.set_compaction_config(CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        recent_messages: 1,
    });
    agent.set_context_window(Some(1_000));
    let mut usage_events = Vec::new();

    agent
        .run_with_events("first".into(), |_| Ok(()))
        .await
        .unwrap();
    agent
        .run_with_events("second".into(), |event| {
            if let AgentEvent::Usage(usage) = event {
                usage_events.push(usage);
            }
            Ok(())
        })
        .await
        .unwrap();

    assert!(usage_events
        .iter()
        .any(|usage| { usage.input_tokens == Some(100) && usage.output_tokens == Some(20) }));
}

#[tokio::test]
async fn compacts_after_tool_results_before_next_provider_call() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: "call_1".into(),
        name: "ok_tool".into(),
        arguments: serde_json::json!({}),
    })]);
    let provider = SequencedProvider {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            response,
            ModelResponse::Assistant(vec![ContentBlock::Text("summary".into())]),
            ModelResponse::Assistant(vec![ContentBlock::Text("recovered".into())]),
        ])),
    };
    let mut tools = ToolRegistry::new();
    tools.register(OkTool);
    let mut agent = test_agent_with_tools(provider, tools);
    agent.set_compaction_config(CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        recent_messages: 1,
    });
    agent.set_context_window(Some(1));

    let output = agent.run("run tool".into()).await.unwrap();

    assert_eq!(output, "recovered");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(matches!(
        requests[1].as_slice(),
        [Message::System(_), Message::User(_)]
    ));
    assert!(requests[2].iter().any(|message| {
        matches!(message, Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text.contains("summary")))
    }));
}

#[tokio::test]
async fn unknown_after_compaction_is_not_overwritten_by_estimate_before_provider_usage() {
    let mut agent = test_agent_with_tools(
        SequencedProvider {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Mutex::new(VecDeque::from(vec![
                ModelResponse::Assistant(vec![ContentBlock::Text("first ok".into())]),
                ModelResponse::Assistant(vec![ContentBlock::Text("summary".into())]),
                ModelResponse::Assistant(vec![ContentBlock::Text("second ok".into())]),
            ])),
        },
        ToolRegistry::new(),
    );
    agent.set_compaction_config(CompactionConfig {
        auto_compact: true,
        threshold_percent: 1,
        recent_messages: 1,
    });
    agent.set_context_window(Some(1_000));
    let mut context_events = Vec::new();

    agent
        .run_with_events("first".into(), |event| {
            if let AgentEvent::ContextUsage(usage) = event {
                context_events.push(usage);
            }
            Ok(())
        })
        .await
        .unwrap();
    agent
        .run_with_events("second".into(), |event| {
            if let AgentEvent::ContextUsage(usage) = event {
                context_events.push(usage);
            }
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(context_events.len(), 2);
    assert_eq!(context_events[0].source, ContextUsageSource::Estimated);
    assert_eq!(context_events[0].context_window, Some(1_000));
    assert_eq!(
        context_events[1].source,
        ContextUsageSource::UnknownAfterCompaction
    );
    assert_eq!(context_events[1].context_window, Some(1_000));
}

#[test]
fn compaction_persists_replacement_history_without_initial_system_message() {
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let mut agent = test_agent(RecordingProvider::default());
    agent.set_history_sink(RecordingHistorySink::replace_target(persisted.clone()));
    agent.messages = vec![
        Message::System("system".into()),
        Message::user_text("summary"),
        Message::user_text("recent"),
    ];

    agent.persist_history_replacement().unwrap();

    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.len(), 2);
    assert!(matches!(persisted[0], Message::User(_)));
    assert!(matches!(persisted[1], Message::User(_)));
}

#[tokio::test]
async fn preserves_history_across_runs() {
    let provider = RecordingProvider::default();
    let requests = provider.requests.clone();
    let mut agent = test_agent(provider);

    agent.run("first".into()).await.unwrap();
    agent.run("second".into()).await.unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(requests[1][0], Message::System(_)));
    assert!(
        matches!(requests[1][1], Message::User(ref blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(s)] if s == "first"))
    );
    assert!(
        matches!(requests[1][2], Message::Assistant(ref blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(s)] if s == "ok"))
    );
    assert!(
        matches!(requests[1][3], Message::User(ref blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(s)] if s == "second"))
    );
}

#[tokio::test]
async fn interrupting_before_tools_leaves_no_unmatched_tool_call() {
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: "call_1".into(),
        name: "ok_tool".into(),
        arguments: serde_json::json!({}),
    })]);
    let provider = RecordingProvider {
        requests: Arc::default(),
        tools: Arc::default(),
        prompt_cache_keys: Arc::default(),
        response: Some(response),
    };
    let mut tools = ToolRegistry::new();
    tools.register(OkTool);
    let mut agent = test_agent_with_tools(provider, tools);
    agent.set_history_sink(RecordingHistorySink::append_target(persisted.clone()));

    let err = agent
        .run_with_events("run tools".into(), |event| match event {
            AgentEvent::ToolStarted { .. } => Err(ModelError::Interrupted),
            _ => Ok(()),
        })
        .await
        .unwrap_err();

    assert!(matches!(err, AgentError::Provider(ModelError::Interrupted)));
    let persisted = persisted.lock().unwrap();
    assert!(persisted
        .iter()
        .all(|message| !matches!(message, Message::Assistant(_) | Message::ToolResult(_))));
}

#[tokio::test]
async fn persists_all_tool_results_before_interrupting_tool_finished_events() {
    let persisted = Arc::new(Mutex::new(Vec::new()));
    let response = ModelResponse::Assistant(vec![
        ContentBlock::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "ok_tool".into(),
            arguments: serde_json::json!({}),
        }),
        ContentBlock::ToolCall(ToolCall {
            id: "call_2".into(),
            name: "ok_tool".into(),
            arguments: serde_json::json!({}),
        }),
    ]);
    let provider = RecordingProvider {
        requests: Arc::default(),
        tools: Arc::default(),
        prompt_cache_keys: Arc::default(),
        response: Some(response),
    };
    let mut tools = ToolRegistry::new();
    tools.register(OkTool);
    let mut agent = test_agent_with_tools(provider, tools);
    agent.set_history_sink(RecordingHistorySink::append_target(persisted.clone()));

    let err = agent
        .run_with_events("run tools".into(), |event| match event {
            AgentEvent::ToolFinished { .. } => Err(ModelError::Interrupted),
            _ => Ok(()),
        })
        .await
        .unwrap_err();

    assert!(matches!(err, AgentError::Provider(ModelError::Interrupted)));
    let persisted = persisted.lock().unwrap();
    let tool_result_count = persisted
        .iter()
        .filter(|message| matches!(message, Message::ToolResult(_)))
        .count();
    assert_eq!(tool_result_count, 2);
}

#[tokio::test]
async fn tool_errors_are_returned_to_model_without_stopping_loop() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: "call_1".into(),
        name: "failing_tool".into(),
        arguments: serde_json::json!({}),
    })]);
    let provider = SequencedProvider {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            response,
            ModelResponse::Assistant(vec![ContentBlock::Text("recovered".into())]),
        ])),
    };
    let mut tools = ToolRegistry::new();
    tools.register(FailingTool);
    let mut agent = test_agent_with_tools(provider, tools);
    let mut tool_events = Vec::new();

    let output = agent
        .run_with_events("run tool".into(), |event| {
            if let AgentEvent::ToolFinished { ok, content, .. } = event {
                tool_events.push((ok, content));
            }
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(output, "recovered");
    assert_eq!(tool_events, vec![(false, "tool failed".into())]);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[1].last(),
        Some(Message::ToolResult(ToolResult { ok: false, content, .. })) if content == "tool failed"
    ));
}

#[tokio::test]
async fn unknown_tools_are_returned_to_model_without_stopping_loop() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: "call_1".into(),
        name: "missing_tool".into(),
        arguments: serde_json::json!({}),
    })]);
    let provider = SequencedProvider {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            response,
            ModelResponse::Assistant(vec![ContentBlock::Text("recovered".into())]),
        ])),
    };
    let mut agent = test_agent_with_tools(provider, ToolRegistry::new());

    let output = agent.run("run tool".into()).await.unwrap();

    assert_eq!(output, "recovered");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[1].last(),
        Some(Message::ToolResult(ToolResult { ok: false, content, .. })) if content == "Unknown tool: missing_tool"
    ));
}

#[test]
fn read_file_event_content_shows_requested_line_range() {
    let cwd = std::env::current_dir().unwrap();
    let content = crate::tools::read_file::ReadFile
        .display_content(
            &serde_json::json!({"path": "src/main.rs", "offset": 10, "limit": 15}),
            &ToolContext {
                cwd,
                max_output_bytes: 12000,
            },
        )
        .unwrap();

    assert_eq!(content, "src/main.rs:10-24");
}

#[test]
fn skill_event_content_shows_skill_name_without_full_content() {
    let cwd = std::env::current_dir().unwrap();
    let content = crate::tools::skill::Skill
        .display_content(
            &serde_json::json!({"name": "caveman"}),
            &ToolContext {
                cwd,
                max_output_bytes: 12000,
            },
        )
        .unwrap();

    assert_eq!(content, "skill caveman");
}

#[test]
fn read_file_event_content_keeps_plain_path_without_range() {
    let cwd = std::env::current_dir().unwrap();
    let content = crate::tools::read_file::ReadFile
        .display_content(
            &serde_json::json!({"path": "src/main.rs"}),
            &ToolContext {
                cwd,
                max_output_bytes: 12000,
            },
        )
        .unwrap();

    assert_eq!(content, "src/main.rs");
}

#[test]
fn read_file_event_content_uses_default_range_bounds() {
    let cwd = std::env::current_dir().unwrap();
    let context = ToolContext {
        cwd,
        max_output_bytes: 12000,
    };

    let from_offset = crate::tools::read_file::ReadFile
        .display_content(
            &serde_json::json!({"path": "src/main.rs", "offset": 10}),
            &context,
        )
        .unwrap();
    let from_limit = crate::tools::read_file::ReadFile
        .display_content(
            &serde_json::json!({"path": "src/main.rs", "limit": 20}),
            &context,
        )
        .unwrap();

    assert_eq!(from_offset, "src/main.rs:10-end");
    assert_eq!(from_limit, "src/main.rs:1-20");
}

#[test]
fn replace_history_keeps_initial_system_message() {
    let mut agent = test_agent(RecordingProvider::default());

    agent.replace_history(vec![
        Message::user_text("previous user"),
        Message::assistant_text("previous assistant"),
    ]);

    assert_eq!(agent.messages.len(), 3);
    assert!(matches!(agent.messages[0], Message::System(_)));
    assert!(
        matches!(agent.messages[1], Message::User(ref blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(s)] if s == "previous user"))
    );
    assert!(
        matches!(agent.messages[2], Message::Assistant(ref blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(s)] if s == "previous assistant"))
    );
}

#[tokio::test]
async fn questionnaire_tool_is_only_advertised_when_handler_is_available() {
    let provider = RecordingProvider::default();
    let tools = provider.tools.clone();
    let mut agent = test_agent(provider);

    agent.run("hello".into()).await.unwrap();

    assert!(!tools.lock().unwrap()[0]
        .iter()
        .any(|tool| tool.name == questionnaire::TOOL_NAME));

    let provider = RecordingProvider::default();
    let tools = provider.tools.clone();
    let mut agent = test_agent(provider);
    let mut ask_questionnaire = |_request: QuestionnaireRequest| -> QuestionnaireFuture {
        panic!("questionnaire handler should not be called")
    };

    agent
        .run_with_content_and_events_questionnaire_and_steering(
            vec![ContentBlock::Text("hello".into())],
            |_| Ok(()),
            Some(&mut ask_questionnaire),
            || Ok(None),
        )
        .await
        .unwrap();

    assert!(tools.lock().unwrap()[0]
        .iter()
        .any(|tool| tool.name == questionnaire::TOOL_NAME));
}

#[tokio::test]
async fn questionnaire_tool_answer_is_returned_to_model() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let tools = Arc::new(Mutex::new(Vec::new()));
    let provider = SequencedToolRecordingProvider {
        requests: requests.clone(),
        tools: tools.clone(),
        responses: Mutex::new(VecDeque::from([
            ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                id: "call_question".into(),
                name: questionnaire::TOOL_NAME.into(),
                arguments: serde_json::json!({
                    "question": "Which file should I edit?",
                    "reason": "The request did not name a file.",
                    "default": "src/main.rs"
                }),
            })]),
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        ])),
    };
    let mut agent = test_agent_with_tools(provider, ToolRegistry::new());
    let mut events = Vec::new();
    let mut ask_questionnaire = |request: QuestionnaireRequest| -> QuestionnaireFuture {
        assert_eq!(request.questions[0].question, "Which file should I edit?");
        let id = request.questions[0].id.clone();
        Box::pin(async move {
            Ok(QuestionnaireResponse {
                answers: vec![QuestionnaireAnswer {
                    id,
                    answer: serde_json::json!("src/lib.rs"),
                }],
            })
        })
    };

    let output = agent
        .run_with_content_and_events_questionnaire_and_steering(
            vec![ContentBlock::Text("edit the file".into())],
            |event| {
                events.push(event);
                Ok(())
            },
            Some(&mut ask_questionnaire),
            || Ok(None),
        )
        .await
        .unwrap();

    assert_eq!(output, "done");
    assert!(tools.lock().unwrap()[0]
        .iter()
        .any(|tool| tool.name == questionnaire::TOOL_NAME));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[1].last(),
        Some(Message::ToolResult(ToolResult { id, ok: true, content }))
            if id == "call_question" && content.contains("src/lib.rs")
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QuestionnaireStarted(request)
            if request.questions[0].question == "Which file should I edit?"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QuestionnaireFinished(response)
            if matches!(response.answers.as_slice(), [QuestionnaireAnswer { answer, .. }] if answer == &serde_json::json!("src/lib.rs"))
    )));
}

#[tokio::test]
async fn questionnaire_tool_multi_question_answers_are_returned_to_model() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = SequencedProvider {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                id: "call_questionnaire".into(),
                name: questionnaire::TOOL_NAME.into(),
                arguments: serde_json::json!({
                    "title": "PR details",
                    "reason": "I need the missing release preferences.",
                    "questions": [
                        {
                            "id": "target_branch",
                            "question": "Which branch should I target?",
                            "type": "choice",
                            "choices": ["main", "develop"],
                            "allow_other": true,
                            "default": "main"
                        },
                        {
                            "id": "test_suites",
                            "question": "Which test suites should I run?",
                            "type": "multi_select",
                            "choices": ["unit", "e2e", "lint"],
                            "default": ["unit", "lint"]
                        },
                        {
                            "id": "include_tests",
                            "question": "Should I include tests?",
                            "type": "confirm",
                            "default": true
                        }
                    ]
                }),
            })]),
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        ])),
    };
    let mut agent = test_agent_with_tools(provider, ToolRegistry::new());
    let mut ask_questionnaire = |request: QuestionnaireRequest| -> QuestionnaireFuture {
        assert_eq!(request.title.as_deref(), Some("PR details"));
        assert_eq!(request.questions.len(), 3);
        assert_eq!(request.questions[0].choices, vec!["main", "develop"]);
        assert!(request.questions[0].allow_other);
        assert_eq!(
            request.questions[1].default,
            Some(serde_json::json!(["unit", "lint"]))
        );
        Box::pin(async move {
            Ok(QuestionnaireResponse {
                answers: vec![
                    QuestionnaireAnswer {
                        id: "target_branch".into(),
                        answer: serde_json::json!("release"),
                    },
                    QuestionnaireAnswer {
                        id: "test_suites".into(),
                        answer: serde_json::json!(["unit", "e2e"]),
                    },
                    QuestionnaireAnswer {
                        id: "include_tests".into(),
                        answer: serde_json::json!("yes"),
                    },
                ],
            })
        })
    };

    let output = agent
        .run_with_content_and_events_questionnaire_and_steering(
            vec![ContentBlock::Text("prep release".into())],
            |_| Ok(()),
            Some(&mut ask_questionnaire),
            || Ok(None),
        )
        .await
        .unwrap();

    assert_eq!(output, "done");
    let requests = requests.lock().unwrap();
    assert!(matches!(
        requests[1].last(),
        Some(Message::ToolResult(ToolResult { id, ok: true, content }))
            if id == "call_questionnaire"
                && content.contains("target_branch")
                && content.contains("release")
                && content.contains("test_suites")
                && content.contains("e2e")
                && content.contains("include_tests")
                && content.contains("yes")
    ));
}

#[tokio::test]
async fn invalid_questionnaire_arguments_return_failed_tool_result() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = SequencedProvider {
        requests: requests.clone(),
        responses: Mutex::new(VecDeque::from([
            ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                id: "call_question".into(),
                name: questionnaire::TOOL_NAME.into(),
                arguments: serde_json::json!({"question": "   "}),
            })]),
            ModelResponse::Assistant(vec![ContentBlock::Text("recovered".into())]),
        ])),
    };
    let mut agent = test_agent_with_tools(provider, ToolRegistry::new());
    let mut ask_questionnaire = |_request: QuestionnaireRequest| -> QuestionnaireFuture {
        panic!("invalid questionnaire should not call handler")
    };

    let output = agent
        .run_with_content_and_events_questionnaire_and_steering(
            vec![ContentBlock::Text("ask".into())],
            |_| Ok(()),
            Some(&mut ask_questionnaire),
            || Ok(None),
        )
        .await
        .unwrap();

    assert_eq!(output, "recovered");
    let requests = requests.lock().unwrap();
    assert!(matches!(
        requests[1].last(),
        Some(Message::ToolResult(ToolResult { ok: false, content, .. }))
            if content == "questions[0].question must not be empty"
    ));
}

#[tokio::test]
async fn empty_tool_registry_sends_no_tool_specs() {
    let provider = RecordingProvider::default();
    let tools = provider.tools.clone();
    let mut agent = test_agent_with_tools(provider, ToolRegistry::new());

    agent.run("hello".into()).await.unwrap();

    let tools = tools.lock().unwrap();
    assert!(tools.last().unwrap().is_empty());
}

#[tokio::test]
async fn without_system_prompt_sends_only_user_message() {
    let provider = RecordingProvider::default();
    let requests = provider.requests.clone();
    let mut agent = test_agent(provider).without_system_prompt();

    assert!(agent.messages().is_empty());

    agent.run("hello".into()).await.unwrap();

    let requests = requests.lock().unwrap();
    let request = requests.last().unwrap();
    assert_eq!(request.len(), 1);
    assert!(matches!(request[0], Message::User(_)));
}

#[test]
fn replace_history_without_system_prompt_keeps_history_only() {
    let mut agent = test_agent(RecordingProvider::default()).without_system_prompt();

    agent.replace_history(vec![Message::user_text("previous user")]);

    assert_eq!(agent.messages().len(), 1);
    assert!(matches!(agent.messages()[0], Message::User(_)));
}

#[tokio::test]
async fn reset_clears_history_back_to_system_prompt() {
    let provider = RecordingProvider::default();
    let requests = provider.requests.clone();
    let mut agent = test_agent(provider);

    agent.run("first".into()).await.unwrap();
    agent.reset();
    agent.run("after reset".into()).await.unwrap();

    let requests = requests.lock().unwrap();
    let last = requests.last().unwrap();
    assert_eq!(last.len(), 2);
    assert!(matches!(last[0], Message::System(_)));
    assert!(
        matches!(last[1], Message::User(ref blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(s)] if s == "after reset"))
    );
}
