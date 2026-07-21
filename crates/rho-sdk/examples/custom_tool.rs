//! Custom tool registration and multi-step tool execution.
//!
//! ```sh
//! cargo run -p rho-sdk --example custom_tool
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{
        AuthorizedToolContext, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind,
        ToolFuture, ToolInvocation, ToolOutput, ToolPreparationContext, ToolPrepareFuture,
    },
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

/// A pure tool that opts in to resource-aware parallel execution.
///
/// An empty resource set means this call can overlap any other resource-aware
/// call up to the runtime limit. Omitting `prepare`, as `ReverseTool` does,
/// keeps a tool exclusive by default.
#[derive(Debug)]
struct LengthTool;

impl Tool for LengthTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "length".into(),
            description: "Count characters in a string argument".into(),
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
        let count = parse_text(&invocation).map(|text| text.chars().count());
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(ToolError::cancelled());
            }
            Ok(ToolOutput::text(count?.to_string()))
        })
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let count = parse_text(&invocation).map(|text| text.chars().count());
        Box::pin(async move {
            let count = count?;
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [],
                Default::default(),
                move |context: AuthorizedToolContext| {
                    Box::pin(async move {
                        if context.cancellation().is_cancelled() {
                            return Err(ToolError::cancelled());
                        }
                        Ok(ToolOutput::text(count.to_string()))
                    })
                },
            ))
        })
    }
}

fn parse_text(invocation: &ToolInvocation) -> Result<&str, ToolError> {
    invocation
        .arguments()
        .get("text")
        .and_then(|value| value.as_str())
        .ok_or_else(|| ToolError::new(ToolErrorKind::InvalidArguments, "missing text argument"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::ToolCall(ToolCall {
                    id: "call-1".into(),
                    name: "reverse".into(),
                    arguments: json!({"text": "rho"}),
                }),
                ContentBlock::ToolCall(ToolCall {
                    id: "call-2".into(),
                    name: "length".into(),
                    arguments: json!({"text": "rho"}),
                }),
            ])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "reversed".into(),
            )])),
        ],
    );
    let rho = Rho::builder()
        .provider(provider)
        .tool(ReverseTool)
        .tool(LengthTool)
        .max_parallel_tools(std::num::NonZeroUsize::new(2).unwrap())
        .build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let outcome = session.complete("reverse rho").await?;
    println!("{}", outcome.text());
    Ok(())
}
