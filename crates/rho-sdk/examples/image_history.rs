//! Image input and explicit in-memory history initialization.

use rho_sdk::{
    model::{ContentBlock, ImageContent, Message, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Rho, SessionOptions, UserInput,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), rho_sdk::Error> {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "example", "vision"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("I received the image.".into()),
        ]))],
    );
    let rho = Rho::builder().provider(provider.clone()).build()?;
    let session = rho
        .session(
            SessionOptions::new().history(vec![Message::user_text("Earlier in-memory context")]),
        )
        .await?;
    let input = UserInput::text_and_images(
        "What is in this image?",
        [ImageContent {
            data: "iVBORw0KGgo=".into(),
            mime_type: "image/png".into(),
        }],
    );

    let mut run = session.start(input).await?;
    while run.next_event().await.is_some() {}
    println!("{}", run.outcome().await?.text());

    let request = &provider.recorded_requests()[0];
    assert_eq!(request.messages.len(), 2);
    assert!(matches!(
        &request.messages[1],
        Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(_), ContentBlock::Image(_)])
    ));
    Ok(())
}
