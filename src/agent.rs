use thiserror::Error;

use crate::model::{Message, ModelError, ModelProvider, ModelRequest, ModelResponse};
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
    #[error("Stopped: maximum step count reached")]
    MaxStepsExceeded,
}

pub struct Agent<P: ModelProvider> {
    provider: P,
    tools: ToolRegistry,
    ctx: ToolContext,
    max_steps: usize,
    messages: Vec<Message>,
}

impl<P: ModelProvider> Agent<P> {
    pub fn new(provider: P, tools: ToolRegistry, ctx: ToolContext, max_steps: usize) -> Self {
        let messages = initial_messages(&tools);
        Self {
            provider,
            tools,
            ctx,
            max_steps,
            messages,
        }
    }

    pub fn reset(&mut self) {
        self.messages = initial_messages(&self.tools);
    }

    pub async fn run(&mut self, user_prompt: String) -> Result<String, AgentError> {
        let specs = self.tools.specs();
        self.messages.push(Message::User(user_prompt));

        for step in 1..=self.max_steps {
            eprintln!("[rho] step {step}/{}", self.max_steps);
            let response = self
                .provider
                .send_turn(ModelRequest {
                    messages: self.messages.clone(),
                    tools: specs.clone(),
                })
                .await?;
            match response {
                ModelResponse::FinalAnswer(answer) => {
                    self.messages.push(Message::Assistant(answer.clone()));
                    return Ok(answer);
                }
                ModelResponse::ToolCall(call) => {
                    self.messages.push(Message::AssistantToolCall(call.clone()));
                    let Some(tool) = self.tools.get(&call.name) else {
                        return Err(AgentError::UnknownTool(call.name));
                    };
                    let result = tool.call(call.arguments, self.ctx.clone(), call.id).await?;
                    println!("[tool:{}]\n{}", call.name, result.content);
                    self.messages.push(Message::ToolResult(result));
                }
            }
        }
        Err(AgentError::MaxStepsExceeded)
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

    #[async_trait]
    impl ModelProvider for RecordingProvider {
        async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            self.requests.lock().unwrap().push(request.messages);
            Ok(ModelResponse::FinalAnswer("ok".into()))
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
            8,
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
        assert!(matches!(requests[1][1], Message::User(ref s) if s == "first"));
        assert!(matches!(requests[1][2], Message::Assistant(ref s) if s == "ok"));
        assert!(matches!(requests[1][3], Message::User(ref s) if s == "second"));
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
        assert!(matches!(last[1], Message::User(ref s) if s == "after reset"));
    }
}
