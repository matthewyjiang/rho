use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    model::{Message, ToolSpec},
    session::{HistoryMetrics, SessionCore},
    CancellationToken, CompactionDecision, CompactionSkipReason, CompactionTrigger,
    ContextEstimate, Error, ProviderError, ProviderErrorKind, RunEvent,
};

use super::{compaction_limit::CompactionLimit, emit, provider_request::ProviderRequestScope};

/// Runs automatic compaction when the policy asks for it, rewriting only the
/// history `limit` allows.
pub(super) async fn maybe_compact(
    core: &Arc<SessionCore>,
    scope: ProviderRequestScope<'_>,
    tool_specs: &[ToolSpec],
    history: &mut Vec<Message>,
    limit: CompactionLimit,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<Option<ContextEstimate>, Error> {
    let estimate = core.advance_context(history, tool_specs);
    let decision = CompactionDecision::evaluate(
        scope.runtime.compaction_policy.as_ref(),
        history.len(),
        estimate,
    )
    .with_extent(limit.extent());
    let Some(compact_end) = limit.compact_end(history.len()) else {
        let decision = decision.blocked_by_pending_tools();
        core.record_compaction_decision(decision);
        if decision.skip_reason() == Some(CompactionSkipReason::PendingAsyncTools) {
            tracing::warn!(
                "skipping compaction: nothing new to compact before pending async tool jobs"
            );
        }
        return Ok(Some(estimate));
    };
    core.record_compaction_decision(decision);
    if decision.skip_reason().is_some() {
        return Ok(Some(estimate));
    }
    // Evaluate the full history above, but give the compactor only the older
    // prefix and that prefix's accounting so a protected suffix cannot distort
    // tail sizing.
    let prefix_estimate = if compact_end == history.len() {
        estimate
    } else {
        core.estimate_context(&history[..compact_end], tool_specs)
    };
    run_compaction(
        core,
        scope,
        tool_specs,
        history,
        compact_end,
        prefix_estimate,
        CompactionTrigger::Automatic,
        cancellation,
        events,
    )
    .await?;
    Ok(None)
}

/// Result of compacting after a context-overflow rejection.
pub(super) enum OverflowRecovery {
    /// History shrank; retry the request.
    Retry,
    /// Recovery is not configured or could not shrink history.
    GiveUp,
}

/// Compacts after the provider rejected the current request as too large.
///
/// Only [`ProviderErrorKind::ContextOverflow`] failures are recovered, and only
/// when the host configured an automatic compaction policy: hosts with manual
/// compaction alone keep their history and receive the original error, as do
/// steps whose `limit` is blocked. History after `limit` is never summarized. Emits
/// `ProviderStreamReset` first so hosts discard any partial attempt output.
#[allow(clippy::too_many_arguments)]
pub(super) async fn recover_context_overflow(
    core: &Arc<SessionCore>,
    scope: ProviderRequestScope<'_>,
    tool_specs: &[ToolSpec],
    history: &mut Vec<Message>,
    limit: CompactionLimit,
    error: &ProviderError,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<OverflowRecovery, Error> {
    let Some(compact_end) = limit.compact_end(history.len()) else {
        return Ok(OverflowRecovery::GiveUp);
    };
    if error.kind() != ProviderErrorKind::ContextOverflow
        || scope.runtime.compaction_policy.is_none()
        || cancellation.is_cancelled()
    {
        return Ok(OverflowRecovery::GiveUp);
    }
    emit(
        events,
        cancellation,
        RunEvent::ProviderStreamReset {
            reason: crate::ProviderStreamResetReason::ContextOverflow,
            detail: "compacting context after the provider rejected an oversized request".into(),
        },
    )
    .await?;
    let estimate = core.estimate_context(&history[..compact_end], tool_specs);
    let outcome = run_compaction(
        core,
        scope,
        tool_specs,
        history,
        compact_end,
        estimate,
        CompactionTrigger::ContextOverflow,
        cancellation,
        events,
    )
    .await?;
    if outcome.current_tokens() < outcome.previous_tokens() {
        Ok(OverflowRecovery::Retry)
    } else {
        Ok(OverflowRecovery::GiveUp)
    }
}

/// Runs the configured compactor over `history[..compact_end]`, commits the
/// replacement plus the protected suffix, and reports both compaction events.
///
/// `tool_specs` are the specs this run's provider turns advertise. The request
/// carries them with the session prompt cache key, so a compactor can reuse the
/// provider's cached prefix.
#[allow(clippy::too_many_arguments)]
async fn run_compaction(
    core: &Arc<SessionCore>,
    scope: ProviderRequestScope<'_>,
    tool_specs: &[ToolSpec],
    history: &mut Vec<Message>,
    compact_end: usize,
    estimate: ContextEstimate,
    trigger: CompactionTrigger,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<crate::CompactionOutcome, Error> {
    let compactor = scope
        .runtime
        .compactor
        .as_ref()
        .expect("builder requires a compactor for automatic policy");
    emit(
        events,
        cancellation,
        RunEvent::CompactionStarted {
            trigger,
            message_count: history.len(),
        },
    )
    .await?;
    let previous = HistoryMetrics::from_history(history);
    let request =
        crate::CompactionRequest::new(history[..compact_end].to_vec(), cancellation.clone())
            .with_context_estimate(estimate)
            .with_trigger(trigger)
            .with_request_context(
                scope.session_id.clone(),
                scope.runtime.usage_parent_session_id.clone(),
                scope.run_id.clone(),
                Some(scope.step_index),
                scope
                    .runtime
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.root().to_path_buf()),
            )
            .with_session_turn(
                scope.runtime.service_tier,
                tool_specs.to_vec(),
                core.prompt_cache_key(),
            );
    let output = match compactor.cancellation_mode() {
        crate::CompactorCancellationMode::Cooperative => compactor.compact(request).await?,
        crate::CompactorCancellationMode::External => {
            tokio::select! {
                result = compactor.compact(request) => result?,
                () = cancellation.cancelled() => return Err(Error::Cancelled),
            }
        }
    };
    let (mut replacement, usage, metadata) = output.into_parts();
    // Do not lose accepted input if the compactor fails or gets cancelled.
    replacement.extend_from_slice(&history[compact_end..]);
    let outcome = core
        .commit_compaction(previous, replacement.clone(), usage, metadata)?
        .with_committed_snapshot(core.persistence_snapshot());
    *history = replacement;
    // Once committed, deliver the checkpoint even if cancellation races with backpressure.
    events
        .send(RunEvent::CompactionCompleted {
            trigger,
            outcome: outcome.clone(),
        })
        .await
        .map_err(|_| Error::Interrupted {
            message: "run event consumer was dropped".into(),
        })?;
    Ok(outcome)
}

#[cfg(test)]
#[path = "overflow_recovery_tests.rs"]
mod tests;
