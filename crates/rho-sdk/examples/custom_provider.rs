//! Minimal custom [`ModelProvider`] implementation.
//!
//! ```sh
//! cargo run -p rho-sdk --example custom_provider
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider, ProviderFuture},
    Rho, SessionOptions,
};

#[derive(Debug)]
struct EchoProvider;

impl ModelProvider for EchoProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("example", "local", "echo")
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Err(rho_sdk::ProviderError::interrupted("cancelled"));
            }
            let prompt = request
                .messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    rho_sdk::model::Message::User(blocks) => blocks.iter().find_map(|block| {
                        if let ContentBlock::Text(text) = block {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    }),
                    _ => None,
                })
                .unwrap_or("empty");
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(format!(
                "echo: {prompt}"
            ))]))
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let rho = Rho::builder().provider(EchoProvider).build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let outcome = session.complete("ping").await?;
    println!("{}", outcome.text());
    Ok(())
}
