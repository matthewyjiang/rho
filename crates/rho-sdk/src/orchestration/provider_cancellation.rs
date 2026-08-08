use crate::{
    model::{ModelEvent, ModelIdentity, ModelUsage},
    provider::{ProviderEventReceiver, ProviderFuture, ProviderRequestEvent, ProviderStreamEvent},
};

use super::stream_capture::{capture_provider_event, StreamCapture};

pub(super) async fn drain_cooperative_provider_on_cancellation(
    future: &mut ProviderFuture<'_>,
    receiver: &mut ProviderEventReceiver,
    identity: &ModelIdentity,
    capture: &mut StreamCapture,
) {
    let mut stream_open = true;
    loop {
        tokio::select! {
            biased;
            event = receiver.recv_timed_stream_event(), if stream_open => {
                match event {
                    Some((ProviderStreamEvent::Model(event), _)) => {
                        capture_model_event(event, identity, capture);
                    }
                    Some((ProviderStreamEvent::Request(
                        ProviderRequestEvent::RequestAttemptFailed { kind, usage }
                    ), _)) => {
                        capture.record_request_attempt_failure(kind, usage);
                    }
                    None => stream_open = false,
                }
            }
            _ = &mut *future => break,
        }
    }
}

pub(super) fn drain_cancelled_provider_events(
    receiver: &mut ProviderEventReceiver,
    identity: &ModelIdentity,
    capture: &mut StreamCapture,
) {
    while let Some((event, _)) = receiver.try_recv_timed_stream_event() {
        match event {
            ProviderStreamEvent::Model(event) => {
                // Cancellation-sensitive host publication must not prevent capture of
                // events the provider had already queued before its future was dropped.
                capture_model_event(event, identity, capture);
            }
            ProviderStreamEvent::Request(ProviderRequestEvent::RequestAttemptFailed {
                kind,
                usage,
            }) => {
                capture.record_request_attempt_failure(kind, usage);
            }
        }
    }
}

fn capture_model_event(event: ModelEvent, identity: &ModelIdentity, capture: &mut StreamCapture) {
    if event.as_generation_output_tokens().is_none() {
        let _ = capture_provider_event(event, identity, &ModelUsage::default(), capture);
    }
}

#[cfg(test)]
#[path = "provider_cancellation_tests.rs"]
mod tests;
