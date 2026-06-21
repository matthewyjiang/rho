use thiserror::Error;

use crate::model::{Message, ModelError, ModelProvider, ModelRequest, ModelResponse, Role};
use crate::prompt::system_prompt;
use crate::tool::{ToolContext, ToolError, ToolRegistry};

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Provider error: {0}")]
    Provider(#[from] ModelError),
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("Invalid tool call: {0}")]
    InvalidToolCall(String),
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
}

impl<P: ModelProvider> Agent<P> {
    pub fn new(provider: P, tools: ToolRegistry, ctx: ToolContext, max_steps: usize) -> Self {
        Self {
            provider,
            tools,
            ctx,
            max_steps,
        }
    }

    pub async fn run(&self, user_prompt: String) -> Result<String, AgentError> {
        let specs = self.tools.specs();
        let mut messages = vec![
            Message {
                role: Role::System,
                content: system_prompt(&specs),
            },
            Message {
                role: Role::User,
                content: user_prompt,
            },
        ];

        for step in 1..=self.max_steps {
            eprintln!("[rho] step {step}/{}", self.max_steps);
            let response = self
                .provider
                .send_turn(ModelRequest {
                    messages: messages.clone(),
                    tools: specs.clone(),
                })
                .await?;
            match response {
                ModelResponse::FinalAnswer(answer) => return Ok(answer),
                ModelResponse::ToolCall(call) => {
                    let Some(tool) = self.tools.get(&call.name) else {
                        return Err(AgentError::UnknownTool(call.name));
                    };
                    let request_text = serde_json::to_string_pretty(&call.arguments)
                        .unwrap_or_else(|_| call.arguments.to_string());
                    messages.push(Message {
                        role: Role::Assistant,
                        content: format!("Tool call: {}\n{}", call.name, request_text),
                    });
                    let result = tool.call(call.arguments, self.ctx.clone(), call.id).await?;
                    let content = serde_json::to_string_pretty(&result)
                        .map_err(|e| AgentError::InvalidToolCall(e.to_string()))?;
                    println!("[tool:{}]\n{}", call.name, result.content);
                    messages.push(Message {
                        role: Role::Tool,
                        content,
                    });
                }
            }
        }
        Err(AgentError::MaxStepsExceeded)
    }
}
