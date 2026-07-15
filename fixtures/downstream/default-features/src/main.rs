use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelRequest, ModelResponse, ToolSpec},
    provider::{ModelProvider, ProviderFuture},
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolOutput},
    Rho, SessionOptions, UserInput,
};

#[derive(Debug)]
struct DownstreamProvider;

impl ModelProvider for DownstreamProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("fixture", "local", "scripted")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async {
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "fixture response".into(),
            )]))
        })
    }
}

#[derive(Debug)]
struct DownstreamTool;

impl Tool for DownstreamTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fixture_tool".into(),
            description: "exercise the downstream tool contract".into(),
            input_schema: Default::default(),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolOutput::text("fixture tool output")) })
    }
}

fn assert_send_sync<T: Send + Sync>() {}

#[allow(dead_code)]
async fn public_completion_and_streaming_contract() -> Result<(), rho_sdk::Error> {
    let rho = Rho::builder()
        .provider(DownstreamProvider)
        .tool(DownstreamTool)
        .build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let _outcome = session.complete("complete").await?;

    let mut run = session.start(UserInput::text("stream")).await?;
    let cancellation = run.cancellation_handle();
    while let Some(_event) = run.next_event().await {}
    let _outcome = run.outcome().await?;
    cancellation.cancel();
    let _history = session.history();
    Ok(())
}

fn main() {
    assert_send_sync::<Rho>();
    assert_send_sync::<DownstreamProvider>();
    let _ = public_completion_and_streaming_contract;
}
