use thiserror::Error;

use crate::model::{
    ContentBlock, DynModelProvider, Message, ModelError, ModelEvent, ModelRequest, ModelResponse,
};
use crate::prompt::system_prompt;
use crate::tool::{ToolContext, ToolError, ToolRegistry, ToolResult};

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Provider error: {0}")]
    Provider(#[from] ModelError),
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    #[error("Message persistence error: {0}")]
    MessagePersistence(#[from] anyhow::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    StepStarted(usize),
    OutputDelta(String),
    ReasoningDelta(String),
    ToolFinished {
        name: String,
        command: Option<String>,
        ok: bool,
        content: String,
    },
}

type MessageSink = Box<dyn FnMut(&Message) -> anyhow::Result<()> + Send>;

pub struct Agent {
    provider: DynModelProvider,
    tools: ToolRegistry,
    ctx: ToolContext,
    messages: Vec<Message>,
    message_sink: Option<MessageSink>,
}

impl Agent {
    pub fn new(provider: DynModelProvider, tools: ToolRegistry, ctx: ToolContext) -> Self {
        let messages = initial_messages(&tools, &ctx.cwd);
        Self {
            provider,
            tools,
            ctx,
            messages,
            message_sink: None,
        }
    }

    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        self.messages.extend(history);
        self
    }

    pub fn set_message_sink(
        &mut self,
        sink: impl FnMut(&Message) -> anyhow::Result<()> + Send + 'static,
    ) {
        self.message_sink = Some(Box::new(sink));
    }

    pub fn clear_message_sink(&mut self) {
        self.message_sink = None;
    }

    pub fn replace_provider(&mut self, provider: DynModelProvider) {
        self.provider = provider;
    }

    pub fn reset(&mut self) {
        self.messages = initial_messages(&self.tools, &self.ctx.cwd);
    }

    pub async fn run(&mut self, user_prompt: String) -> Result<String, AgentError> {
        self.run_with_events(user_prompt, |_| Ok(())).await
    }

    pub fn load_skill(&mut self, skill: &crate::skills::Skill) -> Result<(), AgentError> {
        self.push_message(Message::user_text(format!(
            "Loaded skill `{}` from {}:\n\n{}",
            skill.name,
            skill.path.display(),
            skill.contents
        )))
    }

    fn push_message(&mut self, message: Message) -> Result<(), AgentError> {
        if let Some(sink) = &mut self.message_sink {
            sink(&message)?;
        }
        self.messages.push(message);
        Ok(())
    }

    fn push_skipped_tool_results(
        &mut self,
        tool_calls: &[crate::tool::ToolCall],
    ) -> Result<(), AgentError> {
        for call in tool_calls {
            self.push_message(Message::ToolResult(ToolResult {
                id: call.id.clone(),
                ok: false,
                content: "Skipped because an earlier tool call failed".into(),
            }))?;
        }
        Ok(())
    }

    pub async fn run_with_events(
        &mut self,
        user_prompt: String,
        mut on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
    ) -> Result<String, AgentError> {
        let specs = self.tools.specs();
        self.push_message(Message::user_text(user_prompt))?;

        let mut step = 1usize;
        loop {
            on_event(AgentEvent::StepStarted(step))?;
            let response = match self
                .provider
                .send_turn_stream(
                    ModelRequest {
                        messages: self.messages.clone(),
                        tools: specs.clone(),
                    },
                    &mut |event| match event {
                        ModelEvent::OutputDelta(text) => on_event(AgentEvent::OutputDelta(text)),
                        ModelEvent::ReasoningDelta(text) => {
                            on_event(AgentEvent::ReasoningDelta(text))
                        }
                    },
                )
                .await
            {
                Ok(response) => response,
                Err(ModelError::Interrupted) => return Err(ModelError::Interrupted.into()),
                Err(err) if should_retry_model_error(&err) => {
                    self.push_message(Message::user_text(format!(
                        "The previous assistant response could not be processed by the client. Error: {err}\n\nPlease continue from the last request. If you attempted a tool call, emit valid tool-call JSON that exactly matches the required schema."
                    )))?;
                    step += 1;
                    continue;
                }
                Err(err) => return Err(err.into()),
            };
            match response {
                ModelResponse::Assistant(blocks) => {
                    let tool_calls: Vec<_> = blocks
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::ToolCall(call) => Some(call.clone()),
                            ContentBlock::Text(_) => None,
                        })
                        .collect();
                    if tool_calls.is_empty() {
                        let answer = blocks
                            .into_iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text(text) => Some(text),
                                ContentBlock::ToolCall(_) => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.push_message(Message::assistant_text(answer.clone()))?;
                        return Ok(answer);
                    }

                    self.push_message(Message::Assistant(blocks))?;
                    let mut tool_events = Vec::new();
                    for (index, call) in tool_calls.iter().cloned().enumerate() {
                        let name = call.name.clone();
                        let command = tool_command(&name, &call.arguments);
                        let event_content = tool_event_content(&name, &call.arguments, &self.ctx);
                        let result = match self.tools.get(&call.name) {
                            Some(tool) => match tool
                                .call(call.arguments, self.ctx.clone(), call.id.clone())
                                .await
                            {
                                Ok(result) => result,
                                Err(err) => {
                                    let result = ToolResult {
                                        id: call.id,
                                        ok: false,
                                        content: err.to_string(),
                                    };
                                    self.push_message(Message::ToolResult(result.clone()))?;
                                    self.push_skipped_tool_results(&tool_calls[index + 1..])?;
                                    return Err(AgentError::Tool(err));
                                }
                            },
                            None => {
                                let result = ToolResult {
                                    id: call.id,
                                    ok: false,
                                    content: format!("Unknown tool: {}", call.name),
                                };
                                self.push_message(Message::ToolResult(result))?;
                                self.push_skipped_tool_results(&tool_calls[index + 1..])?;
                                return Err(AgentError::UnknownTool(call.name));
                            }
                        };
                        let display_content =
                            event_content.unwrap_or_else(|| result.content.clone());
                        let ok = result.ok;
                        self.push_message(Message::ToolResult(result))?;
                        tool_events.push(AgentEvent::ToolFinished {
                            name,
                            command,
                            ok,
                            content: display_content,
                        });
                    }
                    for event in tool_events {
                        on_event(event)?;
                    }
                }
            }
            step += 1;
        }
    }
}

fn should_retry_model_error(error: &ModelError) -> bool {
    matches!(error, ModelError::InvalidResponse(_))
}

fn tool_command(name: &str, arguments: &serde_json::Value) -> Option<String> {
    match name {
        "bash" => arguments
            .get("command")
            .and_then(|command| command.as_str())
            .map(str::to_string),
        _ => None,
    }
}

fn tool_event_content(
    name: &str,
    arguments: &serde_json::Value,
    ctx: &ToolContext,
) -> Option<String> {
    match name {
        "read_file" => arguments
            .get("path")
            .and_then(|path| path.as_str())
            .map(|path| read_file_event_content(&ctx.cwd, path, arguments)),
        "edit_file" | "write_file" => arguments
            .get("path")
            .and_then(|path| path.as_str())
            .map(|path| compact_display_path(&ctx.cwd, path)),
        _ => None,
    }
}

fn read_file_event_content(
    cwd: &std::path::Path,
    path: &str,
    arguments: &serde_json::Value,
) -> String {
    let path = compact_display_path(cwd, path);
    let offset = arguments
        .get("offset")
        .and_then(|offset| offset.as_u64())
        .and_then(|offset| usize::try_from(offset).ok());
    let limit = arguments
        .get("limit")
        .and_then(|limit| limit.as_u64())
        .and_then(|limit| usize::try_from(limit).ok());

    if offset.is_none() && limit.is_none() {
        return path;
    }

    let start = offset.unwrap_or(1);
    let end = limit
        .map(|limit| start.saturating_add(limit).saturating_sub(1).to_string())
        .unwrap_or_else(|| "end".into());
    format!("{path}:{start}-{end}")
}

fn compact_display_path(cwd: &std::path::Path, path: &str) -> String {
    let path = crate::tool::resolve_path(cwd, path);
    path.strip_prefix(cwd)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn initial_messages(tools: &ToolRegistry, cwd: &std::path::Path) -> Vec<Message> {
    vec![Message::System(system_prompt(&tools.specs(), cwd))]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::model::{ModelProvider, ModelRequest, ModelResponse};
    use crate::tool::{Tool, ToolCall, ToolSpec};

    #[derive(Clone, Default)]
    struct RecordingProvider {
        requests: Arc<Mutex<Vec<Vec<Message>>>>,
        response: Option<ModelResponse>,
    }

    #[async_trait(?Send)]
    impl ModelProvider for RecordingProvider {
        async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
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

    fn test_agent_with_tools(provider: RecordingProvider, tools: ToolRegistry) -> Agent {
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
                ModelError::InvalidResponse(message) => {
                    ModelError::InvalidResponse(message.clone())
                }
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
    async fn persists_all_tool_results_before_interrupting_tool_events() {
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
            response: Some(response),
        };
        let mut tools = ToolRegistry::new();
        tools.register(OkTool);
        let mut agent = test_agent_with_tools(provider, tools);
        let persisted_for_sink = persisted.clone();
        agent.set_message_sink(move |message| {
            persisted_for_sink.lock().unwrap().push(message.clone());
            Ok(())
        });

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

    #[test]
    fn read_file_event_content_shows_requested_line_range() {
        let cwd = std::env::current_dir().unwrap();
        let content = tool_event_content(
            "read_file",
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
    fn read_file_event_content_keeps_plain_path_without_range() {
        let cwd = std::env::current_dir().unwrap();
        let content = tool_event_content(
            "read_file",
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

        let from_offset = tool_event_content(
            "read_file",
            &serde_json::json!({"path": "src/main.rs", "offset": 10}),
            &context,
        )
        .unwrap();
        let from_limit = tool_event_content(
            "read_file",
            &serde_json::json!({"path": "src/main.rs", "limit": 20}),
            &context,
        )
        .unwrap();

        assert_eq!(from_offset, "src/main.rs:10-end");
        assert_eq!(from_limit, "src/main.rs:1-20");
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
}
