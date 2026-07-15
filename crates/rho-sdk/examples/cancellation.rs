//! Cooperative cancellation of an in-flight run.
//!
//! ```sh
//! cargo run -p rho-sdk --example cancellation
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    Error, Rho, RunEvent, SessionOptions, UserInput,
};

/// Emits one partial delta, then waits until the run is cancelled.
#[derive(Debug)]
struct PartialProvider;

impl ModelProvider for PartialProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("example", "local", "partial")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(rho_sdk::ProviderError::interrupted("cancelled"))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            events
                .send(ModelEvent::OutputDelta("partial".into()))
                .await?;
            request.cancellation.cancelled().await;
            // Partial assistant content is still recovered into session history.
            let _ = ModelResponse::Assistant(vec![ContentBlock::Text("partial".into())]);
            Err(rho_sdk::ProviderError::interrupted("cancelled"))
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let rho = Rho::builder().provider(PartialProvider).build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let mut run = session.start(UserInput::text("start work")).await?;

    while let Some(event) = run.next_event().await {
        if matches!(event, RunEvent::AssistantTextDelta { .. }) {
            run.cancellation_handle().cancel();
            break;
        }
    }
    while run.next_event().await.is_some() {}

    match run.outcome().await {
        Err(Error::Cancelled) => {
            println!("run cancelled");
            println!("session_running={}", session.is_running());
            println!("history_len={}", session.history().len());
            Ok(())
        }
        Ok(outcome) => {
            eprintln!("expected cancellation, got {}", outcome.text());
            Err(Error::Interrupted {
                message: "run completed instead of cancelling".into(),
            })
        }
        Err(error) => Err(error),
    }
}
