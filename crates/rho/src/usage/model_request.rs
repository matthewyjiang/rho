use std::num::NonZeroUsize;

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse, ModelUsage},
    provider::{provider_event_channel, ModelProvider},
    ProviderError, ProviderRequestOutcome, ProviderRequestUsageContext, ProviderRequestUsageEvent,
    ProviderRequestUsageRecording,
};

const EVENT_CAPACITY: usize = 16;

/// Executes and durably accounts for a provider request outside the agent loop.
pub(crate) async fn send_recorded(
    provider: &dyn ModelProvider,
    request: ModelRequest<'_>,
    context: ProviderRequestUsageContext,
    recording: ProviderRequestUsageRecording,
) -> Result<(ModelResponse, ModelUsage), ProviderError> {
    let cancellation = request.cancellation.clone();
    let (events, mut receiver) =
        provider_event_channel(NonZeroUsize::new(EVENT_CAPACITY).expect("capacity is nonzero"));
    let provider_call = provider.send_turn_stream(request, events);
    let collect_usage = async {
        let mut usage = ModelUsage::default();
        let mut failed_attempts = Vec::new();
        while let Some(event) = receiver.recv().await {
            match event {
                ModelEvent::Usage(partial) => usage = usage.saturating_add(&partial),
                ModelEvent::RequestAttemptFailed {
                    kind,
                    usage: attempt_usage,
                } => {
                    failed_attempts.push((kind, usage.saturating_add(&attempt_usage)));
                    usage = ModelUsage::default();
                }
                ModelEvent::OutputDelta(_)
                | ModelEvent::ReasoningDelta(_)
                | ModelEvent::ReasoningSummaryDelta(_)
                | ModelEvent::WebSearch(_)
                | ModelEvent::ToolCallDelta { .. }
                | ModelEvent::ProviderContext { .. } => {}
            }
        }
        (usage, failed_attempts)
    };
    let (result, (usage, failed_attempts)) = tokio::join!(provider_call, collect_usage);
    let outcome = match &result {
        Ok(_) => ProviderRequestOutcome::Completed,
        Err(_) if cancellation.is_cancelled() => ProviderRequestOutcome::Cancelled,
        Err(error) => ProviderRequestOutcome::Failed(error.kind()),
    };
    let mut next_attempt_index = 1;
    for (kind, usage) in failed_attempts {
        recording
            .record(ProviderRequestUsageEvent::observed(
                context.clone().with_attempt_index(next_attempt_index),
                usage,
                ProviderRequestOutcome::Failed(kind),
            ))
            .await;
        next_attempt_index += 1;
    }
    recording
        .record(ProviderRequestUsageEvent::observed(
            context.with_attempt_index(next_attempt_index),
            usage.clone(),
            outcome,
        ))
        .await;
    result.map(|response| (response, usage))
}
