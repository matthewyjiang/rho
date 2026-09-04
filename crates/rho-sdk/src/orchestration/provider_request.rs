use crate::{
    model::{Message, ModelResponse, ModelUsage},
    ProviderError, ProviderErrorKind, Retryability, RunEvent,
};

use super::{
    emit, provider_turn, stream_capture::StreamCapture, Rho, RunControl, INVALID_RESPONSE_ATTEMPTS,
    MAX_HONORED_RETRY_AFTER, PROVIDER_TURN_ATTEMPTS, RETRYABLE_REQUEST_BASE_DELAY,
};

pub(super) struct RequestFailure {
    pub error: ProviderError,
    pub capture: StreamCapture,
}

impl RequestFailure {
    pub(super) fn boxed(error: ProviderError, capture: StreamCapture) -> Box<Self> {
        Box::new(Self { error, capture })
    }
}

#[derive(Clone, Copy)]
pub(super) struct ProviderRequestScope<'a> {
    pub runtime: &'a Rho,
    pub session_id: &'a crate::SessionId,
    pub run_id: &'a crate::RunId,
    pub step_index: usize,
}

pub(super) async fn request_valid_response(
    scope: ProviderRequestScope<'_>,
    history: &[Message],
    tools: &[crate::model::ToolSpec],
    accumulated_usage: &ModelUsage,
    reasoning_level: crate::ReasoningLevel,
    prompt_cache_key: Option<&str>,
    control: &mut RunControl<'_>,
) -> Result<(ModelResponse, StreamCapture), Box<RequestFailure>> {
    let mut next_attempt_index = 1;
    let mut provider_turn_attempts = 0;
    let mut invalid_responses = 0;
    let mut failed_requests = 0;
    loop {
        provider_turn_attempts += 1;
        let result = provider_turn(
            scope.runtime,
            history,
            tools,
            accumulated_usage,
            reasoning_level,
            prompt_cache_key,
            control,
        )
        .await;
        let (response, capture) = match result {
            Ok((response, mut capture)) => {
                next_attempt_index =
                    record_failed_provider_attempts(&scope, next_attempt_index, &mut capture).await;
                let outcome = if response.protocol_issue().is_none() {
                    crate::ProviderRequestOutcome::Completed
                } else {
                    crate::ProviderRequestOutcome::InvalidResponse
                };
                record_request_usage(&scope, next_attempt_index, capture.usage().clone(), outcome)
                    .await;
                next_attempt_index += 1;
                (response, capture)
            }
            Err(mut failure) => {
                next_attempt_index = record_failed_provider_attempts(
                    &scope,
                    next_attempt_index,
                    &mut failure.capture,
                )
                .await;
                let outcome = if control.cancellation.is_cancelled() {
                    crate::ProviderRequestOutcome::Cancelled
                } else {
                    crate::ProviderRequestOutcome::Failed(failure.error.kind())
                };
                record_request_usage(
                    &scope,
                    next_attempt_index,
                    failure.capture.usage().clone(),
                    outcome,
                )
                .await;
                next_attempt_index += 1;
                failed_requests += 1;
                if control.cancellation.is_cancelled()
                    || !failure.error.is_retryable()
                    || provider_turn_attempts >= PROVIDER_TURN_ATTEMPTS
                {
                    return Err(failure);
                }
                let detail = format!(
                    "retrying after provider attempt {provider_turn_attempts} of {PROVIDER_TURN_ATTEMPTS}: {}",
                    failure.error.message()
                );
                let retry_after = failure.error.retry_after();
                let _ = emit(
                    control.events,
                    control.cancellation,
                    RunEvent::ProviderStreamReset {
                        reason: crate::ProviderStreamResetReason::RetryableFailure {
                            kind: failure.error.kind(),
                            retry_after: retry_after.filter(|delay| !delay.is_zero()),
                        },
                        detail,
                    },
                )
                .await;
                let exponential =
                    RETRYABLE_REQUEST_BASE_DELAY * 2u32.pow(failed_requests as u32 - 1);
                // Honor a provider wait when present. Cap so a multi-hour
                // Retry-After cannot stall the interactive session; the final
                // error still carries the full provider hint.
                let delay = match retry_after {
                    Some(wait) if !wait.is_zero() => {
                        wait.min(MAX_HONORED_RETRY_AFTER).max(exponential)
                    }
                    _ => exponential,
                };
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = control.cancellation.cancelled() => {
                        // ProviderStreamReset already abandoned this attempt.
                        // Do not commit its discarded partials as AbortedAssistant.
                        failure.capture = StreamCapture::default();
                        return Err(failure);
                    }
                }
                continue;
            }
        };
        let Some(issue) = response.protocol_issue() else {
            return Ok((response, capture));
        };
        invalid_responses += 1;
        if invalid_responses >= INVALID_RESPONSE_ATTEMPTS
            || provider_turn_attempts >= PROVIDER_TURN_ATTEMPTS
        {
            // Invalid attempts are discarded; do not install stream fragments
            // into session history on the terminal failure path.
            return Err(RequestFailure::boxed(
                ProviderError::new(
                    ProviderErrorKind::InvalidResponse,
                    issue,
                    Retryability::Permanent,
                ),
                StreamCapture::default(),
            ));
        }
        let detail = format!(
            "retrying malformed provider response after provider attempt {provider_turn_attempts} of {PROVIDER_TURN_ATTEMPTS}"
        );
        let _ = emit(
            control.events,
            control.cancellation,
            RunEvent::ProviderStreamReset {
                reason: crate::ProviderStreamResetReason::InvalidResponse,
                detail,
            },
        )
        .await;
    }
}

async fn record_failed_provider_attempts(
    scope: &ProviderRequestScope<'_>,
    mut next_attempt_index: usize,
    capture: &mut StreamCapture,
) -> usize {
    for (kind, usage) in capture.take_failed_attempts() {
        record_request_usage(
            scope,
            next_attempt_index,
            usage,
            crate::ProviderRequestOutcome::Failed(kind),
        )
        .await;
        next_attempt_index += 1;
    }
    next_attempt_index
}

async fn record_request_usage(
    scope: &ProviderRequestScope<'_>,
    attempt_index: usize,
    usage: ModelUsage,
    outcome: crate::ProviderRequestOutcome,
) {
    let mut context = crate::ProviderRequestUsageContext::new(
        scope.runtime.provider.identity(),
        scope.session_id.clone(),
        scope.run_id.clone(),
        scope.step_index,
        attempt_index,
        scope
            .runtime
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().to_path_buf()),
        scope.runtime.usage_purpose.clone(),
    );
    if let Some(parent_session_id) = &scope.runtime.usage_parent_session_id {
        context = context.with_parent_session_id(parent_session_id.clone());
    }
    scope
        .runtime
        .usage_recording
        .record(crate::ProviderRequestUsageEvent::observed(
            context, usage, outcome,
        ))
        .await;
}
