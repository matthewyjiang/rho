use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    model::{Message, ToolSpec},
    session::{HistoryMetrics, SessionCore},
    CancellationToken, CompactionDecision, ContextEstimate, Error, RunEvent,
};

use super::{emit, provider_request::ProviderRequestScope};

pub(super) async fn maybe_compact(
    core: &Arc<SessionCore>,
    scope: ProviderRequestScope<'_>,
    tool_specs: &[ToolSpec],
    history: &mut Vec<Message>,
    preserve_from: Option<usize>,
    cancellation: &CancellationToken,
    events: &mpsc::Sender<RunEvent>,
) -> Result<Option<ContextEstimate>, Error> {
    let estimate = core.advance_context(history, tool_specs);
    let decision = CompactionDecision::evaluate(
        scope.runtime.compaction_policy.as_ref(),
        history.len(),
        estimate,
    );
    core.record_compaction_decision(decision);
    if decision.skip_reason().is_some() {
        return Ok(Some(estimate));
    }
    let compactor = scope
        .runtime
        .compactor
        .as_ref()
        .expect("builder requires a compactor for automatic policy");
    emit(
        events,
        cancellation,
        RunEvent::CompactionStarted {
            trigger: crate::CompactionTrigger::Automatic,
            message_count: history.len(),
        },
    )
    .await?;
    let previous = HistoryMetrics::from_history(history);
    // Fresh completion input must reach one provider request verbatim. Evaluate
    // the full history above, but give the compactor only the older prefix and
    // that prefix's accounting so a protected suffix cannot distort tail sizing.
    let compact_end = preserve_from.unwrap_or(history.len());
    let prefix_estimate = if compact_end == history.len() {
        estimate
    } else {
        core.estimate_context(&history[..compact_end], tool_specs)
    };
    let request =
        crate::CompactionRequest::new(history[..compact_end].to_vec(), cancellation.clone())
            .with_context_estimate(prefix_estimate)
            .with_trigger(crate::CompactionTrigger::Automatic)
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
            );
    let request = match scope.runtime.service_tier {
        Some(tier) => request.with_service_tier(tier),
        None => request,
    };
    let output = match compactor.cancellation_mode() {
        crate::CompactorCancellationMode::Cooperative => compactor.compact(request).await?,
        crate::CompactorCancellationMode::External => {
            tokio::select! {
                result = compactor.compact(request) => result?,
                () = cancellation.cancelled() => return Err(Error::Cancelled),
            }
        }
    };
    let (mut replacement, usage) = output.into_parts();
    // Do not lose accepted input if the compactor fails or gets cancelled.
    replacement.extend_from_slice(&history[compact_end..]);
    let outcome = core
        .commit_compaction(previous, replacement.clone(), usage)?
        .with_committed_snapshot(core.persistence_snapshot());
    *history = replacement;
    // Once committed, deliver the checkpoint even if cancellation races with backpressure.
    events
        .send(RunEvent::CompactionCompleted {
            trigger: crate::CompactionTrigger::Automatic,
            outcome,
        })
        .await
        .map_err(|_| Error::Interrupted {
            message: "run event consumer was dropped".into(),
        })?;
    Ok(None)
}
