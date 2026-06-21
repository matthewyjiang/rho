use thiserror::Error;

use crate::model::{
    ContentBlock, Message, ModelError, ModelEvent, ModelProvider, ModelRequest, ModelResponse,
};
use crate::prompt::system_prompt;
use crate::tool::{ToolContext, ToolError, ToolRegistry};

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Provider error: {0}")]
    Provider(#[from] ModelError),
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
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

pub struct Agent<P: ModelProvider> {
    provider: P,
    tools: ToolRegistry,
    ctx: ToolContext,
    messages: Vec<Message>,
}

impl<P: ModelProvider> Agent<P> {
    pub fn new(provider: P, tools: ToolRegistry, ctx: ToolContext) -> Self {
        let messages = initial_messages(&tools);
        Self {
            provider,
            tools,
            ctx,
            messages,
        }
    }

    pub fn reset(&mut self) {
        self.messages = initial_messages(&self.tools);
    }

    pub async fn run(&mut self, user_prompt: String) -> Result<String, AgentError> {
        self.run_with_events(user_prompt, |_| Ok(())).await
    }

    pub async fn run_with_events(
        &mut self,
        user_prompt: String,
        mut on_event: impl FnMut(AgentEvent) -> Result<(), ModelError>,
    ) -> Result<String, AgentError> {
        let specs = self.tools.specs();
        self.messages.push(Message::user_text(user_prompt));

        let mut step = 1usize;
        loop {
            on_event(AgentEvent::StepStarted(step))?;
            let response = self
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
                .await?;
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
                        self.messages.push(Message::assistant_text(answer.clone()));
                        return Ok(answer);
                    }

                    self.messages.push(Message::Assistant(blocks));
                    for call in tool_calls {
                        let Some(tool) = self.tools.get(&call.name) else {
                            return Err(AgentError::UnknownTool(call.name));
                        };
                        let name = call.name.clone();
                        let command = tool_command(&name, &call.arguments);
                        let event_content = tool_event_content(&name, &call.arguments);
                        let result = tool.call(call.arguments, self.ctx.clone(), call.id).await?;
                        on_event(AgentEvent::ToolFinished {
                            name,
                            command,
                            ok: result.ok,
                            content: event_content.unwrap_or_else(|| result.content.clone()),
                        })?;
                        self.messages.push(Message::ToolResult(result));
                    }
                }
            }
            step += 1;
        }
    }
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

fn tool_event_content(name: &str, arguments: &serde_json::Value) -> Option<String> {
    match name {
        "read_file" => arguments
            .get("path")
            .and_then(|path| path.as_str())
            .map(|path| format!("read {path}")),
        _ => None,
    }
}

fn initial_messages(tools: &ToolRegistry) -> Vec<Message> {
    vec![Message::System(system_prompt(&tools.specs()))]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;
    use crate::model::{ModelRequest, ModelResponse};

    #[derive(Clone, Default)]
    struct RecordingProvider {
        requests: Arc<Mutex<Vec<Vec<Message>>>>,
    }

    #[async_trait(?Send)]
    impl ModelProvider for RecordingProvider {
        async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            self.requests.lock().unwrap().push(request.messages);
            Ok(ModelResponse::final_answer("ok"))
        }
    }

    fn test_agent(provider: RecordingProvider) -> Agent<RecordingProvider> {
        Agent::new(
            provider,
            ToolRegistry::new(),
            ToolContext {
                cwd: std::env::current_dir().unwrap(),
                max_output_bytes: 12000,
            },
        )
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
