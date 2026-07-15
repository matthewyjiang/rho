//! Portable session snapshots without SQLite.
//!
//! ```sh
//! cargo run -p rho-sdk --example session_snapshot
//! ```

use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    InMemorySessionStore, Rho, SessionOptions,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let store = InMemorySessionStore::new();

    let first = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("first turn".into()),
            ]))],
        ))
        .build()?;
    let session = first.session(SessionOptions::default()).await?;
    let first_outcome = session.complete("remember this").await?;
    let snapshot = session.snapshot().with_metadata("host", "example");
    store.save(snapshot.clone());
    println!(
        "saved session={} revision={}",
        snapshot.session_id(),
        snapshot.revision().get()
    );

    let restored_snapshot = store
        .load(snapshot.session_id())
        .expect("snapshot should be present");
    let second = Rho::builder()
        .provider(ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("second turn".into()),
            ]))],
        ))
        .build()?;
    let restored = second
        .session(SessionOptions::from_snapshot(restored_snapshot))
        .await?;
    let second_outcome = restored.complete("continue").await?;

    println!("first={}", first_outcome.text());
    println!("second={}", second_outcome.text());
    println!(
        "history_messages={} revision={}",
        restored.history().len(),
        second_outcome.revision().get()
    );
    Ok(())
}
