//! Final-answer completion without consuming the event stream.
//!
//! ```sh
//! cargo run -p rho-sdk --example simple_completion
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Rho, SessionOptions,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("hello from rho-sdk".into()),
        ]))],
    );
    let rho = Rho::builder().provider(provider).build()?;
    let session = rho.session(SessionOptions::default()).await?;

    let outcome = session.complete("say hello").await?;
    println!("{}", outcome.text());
    println!("revision={}", outcome.revision().get());
    Ok(())
}
