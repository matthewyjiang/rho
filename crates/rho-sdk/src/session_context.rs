use crate::{
    model::{Message, ModelIdentity, ModelUsage, ToolSpec},
    CompactionDecision, ContextEstimate,
};

use super::{Session, SessionCore, SessionData};

impl Session {
    /// Committed compaction accounting, including the last local-token result.
    ///
    /// Unlike a policy decision, this records completed operations, including
    /// unchanged results. It survives snapshot restore and does not copy history.
    pub fn compaction_state(&self) -> crate::CompactionState {
        self.core
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .compaction
            .clone()
    }

    /// Current committed context when idle, or the latest in-flight history boundary.
    ///
    /// Updated before `StepStarted`, after a successful provider response, and at
    /// commit. Stream deltas and provisional usage do not establish a baseline:
    /// that request may still fail. Revalidates provider identity and tool schemas
    /// on each read. While idle, also samples request-only context and refreshes
    /// accounting when it changes; unchanged reads do not scan or copy history.
    pub fn context_estimate(&self) -> ContextEstimate {
        let runtime = self.core.runtime();
        let tools = runtime.advertised_tool_specs();
        let source = (!self.is_running()).then(|| runtime.context_messages(self.id()));
        let identity = runtime.provider.identity();
        let mut data = self
            .core
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let data = &mut *data;
        if let Some(context) = &mut data.working_context {
            context.current(&tools, &identity)
        } else if let Some(source) = source {
            data.context
                .advance(&data.history, &source, &tools, &identity)
        } else {
            data.context.current(&tools, &identity)
        }
    }

    /// Estimates proposed history using the current provider and tool schemas.
    ///
    /// Hosts can append pending input to their history before starting a run.
    /// Includes a fresh sample of request-only context. Calibration applies only
    /// when that source matches the measured request and its raw history remains
    /// an unchanged prefix; replacements get a provider-neutral estimate instead.
    /// This scans the supplied history without copying it or mutating session state.
    pub fn estimate_context(&self, messages: &[Message]) -> ContextEstimate {
        let runtime = self.core.runtime();
        let tools = runtime.advertised_tool_specs();
        self.core.estimate_context(messages, &tools)
    }

    /// Latest automatic policy check, including skipped checks. Not persisted.
    pub fn last_compaction_decision(&self) -> Option<CompactionDecision> {
        self.core
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last_compaction_decision
    }
}

impl SessionCore {
    /// Estimates proposed history without changing the active accounting.
    pub(crate) fn estimate_context(
        &self,
        history: &[Message],
        tools: &[ToolSpec],
    ) -> ContextEstimate {
        let runtime = self.runtime();
        let source = runtime.context_messages(self.id());
        let identity = runtime.provider.identity();
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.working_context
            .as_ref()
            .unwrap_or(&data.context)
            .estimate(history, &source, tools, &identity)
    }

    /// Prepares host-owned context using current boundary accounting. Sampling
    /// and accounting finish before the host callback, with no session lock held.
    /// Callers must sample the source again afterward to use the prepared context.
    pub(crate) fn prepare_request_context(&self, history: &[Message], tools: &[ToolSpec]) {
        let runtime = self.runtime();
        let Some(source) = &runtime.request_context else {
            return;
        };
        let context = source.messages(self.id());
        let estimate = self.advance_request_context(history, &context, tools);
        source.prepare(self.id(), history, tools, estimate);
    }

    /// Prepares request-only context, then publishes accounting for the
    /// append-only working history before automatic compaction evaluation.
    pub(crate) fn advance_context(
        &self,
        history: &[Message],
        tools: &[ToolSpec],
    ) -> ContextEstimate {
        self.prepare_request_context(history, tools);
        let runtime = self.runtime();
        let source = runtime.context_messages(self.id());
        self.advance_request_context(history, &source, tools)
    }

    /// Reuses boundary accounting when the immutable request source is unchanged.
    pub(crate) fn advance_request_context(
        &self,
        history: &[Message],
        source: &[Message],
        tools: &[ToolSpec],
    ) -> ContextEstimate {
        let runtime = self.runtime();
        let identity = runtime.provider.identity();
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        working_context(&mut data).advance(history, source, tools, &identity)
    }

    pub(crate) fn record_context_usage(
        &self,
        history: &[Message],
        source: &[Message],
        tools: &[ToolSpec],
        usage: &ModelUsage,
        request_estimate: ContextEstimate,
    ) {
        let identity = self.runtime().provider.identity();
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        working_context(&mut data).record(
            history,
            source,
            tools,
            identity,
            usage,
            request_estimate,
        );
    }

    pub(crate) fn invalidate_working_context(&self) {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        working_context(&mut data).invalidate();
    }

    pub(crate) fn append_context_estimate(&self, message: &Message) {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(context) = &mut data.working_context {
            context.append(message);
        }
    }

    pub(crate) fn invalidate_context(&self) {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.context.invalidate();
        data.working_context = None;
        data.last_compaction_decision = None;
    }

    pub(crate) fn record_compaction_decision(&self, decision: CompactionDecision) {
        tracing::debug!(
            estimated_tokens = decision.estimate().estimated_tokens(),
            context_tokens = decision.estimate().tokens(),
            provider_reported_tokens = decision.estimate().provider_reported_tokens(),
            provider_request_estimated_tokens = decision.estimate().provider_request_estimated_tokens(),
            threshold = ?decision.threshold(),
            skip_reason = ?decision.skip_reason(),
            "automatic compaction policy check"
        );
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last_compaction_decision = Some(decision);
    }
}

fn working_context(data: &mut SessionData) -> &mut crate::context_estimate::ContextAccounting {
    data.working_context
        .get_or_insert_with(|| data.context.clone())
}

pub(super) fn commit_context(
    data: &mut SessionData,
    history: &[Message],
    source: &[Message],
    tools: &[ToolSpec],
    identity: &ModelIdentity,
) {
    let mut context = data
        .working_context
        .take()
        .unwrap_or_else(|| data.context.clone());
    context.advance(history, source, tools, identity);
    data.context = context;
}

pub(super) fn commit_replacement(
    data: &mut SessionData,
    history: &[Message],
    source: &[Message],
    tools: &[ToolSpec],
    identity: &ModelIdentity,
) {
    let mut context = data
        .working_context
        .take()
        .unwrap_or_else(|| data.context.clone());
    context.replace(history, source, tools, identity);
    data.context = context;
}
