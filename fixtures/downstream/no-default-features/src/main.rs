use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Rho, SessionOptions,
};

#[allow(dead_code)]
async fn minimal_surface_contract() -> Result<(), rho_sdk::Error> {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("fixture", "local", "scripted"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("minimal response".into()),
        ]))],
    );
    let rho = Rho::builder().provider(provider).build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let outcome = session.complete("hello").await?;
    assert_eq!(outcome.text(), "minimal response");
    Ok(())
}

fn main() {
    let _ = minimal_surface_contract;
}
