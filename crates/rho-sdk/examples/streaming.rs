//! Ordered semantic events with a typed final outcome.
//!
//! ```sh
//! cargo run -p rho-sdk --example streaming
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Rho, RunEvent, SessionOptions, UserInput,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [ScriptedTurn::streaming(
            vec![
                ModelEvent::OutputDelta("hel".into()),
                ModelEvent::OutputDelta("lo".into()),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())]),
        )],
    );
    let rho = Rho::builder().provider(provider).build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let mut run = session.start(UserInput::text("stream hello")).await?;

    while let Some(event) = run.next_event().await {
        match event {
            RunEvent::AssistantTextDelta { text } => print!("{text}"),
            RunEvent::Completed { outcome } => {
                println!();
                println!("final={}", outcome.text());
            }
            RunEvent::Failed { message, .. } => {
                eprintln!("failed: {message}");
            }
            _ => {}
        }
    }

    // The same typed result is available without reconstructing deltas.
    let outcome = run.outcome().await?;
    assert_eq!(outcome.text(), "hello");
    Ok(())
}
