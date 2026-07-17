use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse, ModelUsage},
    provider::{provider_event_channel, ModelProvider},
    ProviderError,
};

use super::{RequestOutcome, SqliteUsageRecorder, UsageEvent, UsageRecorder};

const EVENT_CAPACITY: usize = 16;
static WRITE_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

/// Executes and durably accounts for a non-agent provider request.
pub(crate) async fn send_recorded(
    provider: &dyn ModelProvider,
    request: ModelRequest<'_>,
    purpose: &'static str,
    recorder: Option<Arc<SqliteUsageRecorder>>,
) -> Result<(ModelResponse, ModelUsage), ProviderError> {
    let identity = provider.identity();
    let cancellation = request.cancellation.clone();
    let (events, mut receiver) =
        provider_event_channel(NonZeroUsize::new(EVENT_CAPACITY).expect("capacity is nonzero"));
    let provider_call = provider.send_turn_stream(request, events);
    let collect_usage = async {
        let mut usage = ModelUsage::default();
        while let Some(event) = receiver.recv().await {
            if let ModelEvent::Usage(partial) = event {
                usage = usage.saturating_add(&partial);
            }
        }
        usage
    };
    let (result, usage) = tokio::join!(provider_call, collect_usage);
    let outcome = match &result {
        Ok(_) => RequestOutcome::Completed,
        Err(_) if cancellation.is_cancelled() => RequestOutcome::Cancelled,
        Err(_) => RequestOutcome::Failed,
    };
    if let Some(recorder) = recorder {
        let event = UsageEvent::new(
            identity.provider,
            identity.model,
            purpose,
            outcome,
            usage.clone(),
        );
        let write = tokio::task::spawn_blocking(move || UsageRecorder::record(&*recorder, &event));
        let failure = match write.await {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(error) => Some(format!("usage ledger task failed: {error}")),
        };
        match failure {
            Some(error) if !WRITE_WARNING_EMITTED.swap(true, Ordering::Relaxed) => {
                eprintln!("warning: model response usage could not be recorded: {error}");
            }
            Some(_) | None => {}
        }
    }
    result.map(|response| (response, usage))
}
