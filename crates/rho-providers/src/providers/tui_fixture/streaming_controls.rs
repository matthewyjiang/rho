//! Marker-gated output with ordered, visible receipts independent of text visibility.

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse, ModelUsage},
    provider::ProviderEventSender,
    ProviderError,
};

pub(super) async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    const RELEASE: &str = ".rho-fixture-release-streaming-controls";
    super::release::consume_release(RELEASE)?;
    let mut response = String::new();
    for delta in [
        "Hidden off prefix",
        " continued.\n\nParagraph checkpoint visible.\n\nHeld paragraph suffix",
        "\n\n``",
        // Keep the prose in live preview: committed history can reparse and
        // conceal a broken fence tracker, but preview would expose raw **.
        "`rust\nlet streamed_value = 7;\n```\n\n**Markdown continuation complete.**",
    ] {
        events.send(ModelEvent::OutputDelta(delta.into())).await?;
        response.push_str(delta);
        // Usage follows the delta on the same ordered event stream. The PTY
        // waits for this context count before checking hidden text, so absence
        // never depends on how quickly the provider or terminal was scheduled.
        // Unlike reasoning/tool events, usage does not finalize Markdown.
        // Usage events accumulate, yielding distinct 11/22/33/44K receipts.
        events
            .send(ModelEvent::Usage(ModelUsage {
                input_tokens: Some(11_000),
                context_window: Some(100_000),
                ..ModelUsage::default()
            }))
            .await?;
        super::release::wait_for_release_or_cancel(RELEASE, &request.cancellation).await?;
    }
    let ending = "\nBuffered completion delivered.";
    events.send(ModelEvent::OutputDelta(ending.into())).await?;
    response.push_str(ending);
    super::completed(response)
}
