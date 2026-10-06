//! Hold a successful turn until multiple gated follow-ups are queued by the PTY.

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

pub(super) async fn stream(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    const RELEASE: &str = ".rho-fixture-release-send-confirm";
    super::release::consume_release(RELEASE)?;
    events
        .send(ModelEvent::OutputDelta(format!(
            "queue checkpoint: {prompt}"
        )))
        .await?;
    super::release::wait_for_release_or_cancel(RELEASE, &request.cancellation).await?;
    super::completed(format!("queue released: {prompt}"))
}
