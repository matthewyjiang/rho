//! Custom tool registration and multi-step tool execution.
//!
//! ```sh
//! cargo run -p rho-sdk --example custom_tool
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput},
    Rho, SessionOptions,
};
use serde_json::json;

#[derive(Debug)]
struct ReverseTool;

impl Tool for ReverseTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "reverse".into(),
            description: "Reverse a string argument".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            let text = invocation
                .arguments()
                .get("text")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    ToolError::new(ToolErrorKind::InvalidArguments, "missing text argument")
                })?;
            let reversed: String = text.chars().rev().collect();
            Ok(ToolOutput::text(reversed))
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "reverse".into(),
                    arguments: json!({"text": "rho"}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "reversed".into(),
            )])),
        ],
    );
    let rho = Rho::builder()
        .provider(provider)
        .tool(ReverseTool)
        .build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let outcome = session.complete("reverse rho").await?;
    println!("{}", outcome.text());
    Ok(())
}
