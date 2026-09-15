use std::{collections::hash_map::DefaultHasher, hash::Hasher, io::Write};

use crate::model::{
    context::estimate_context_tokens, Message, ModelIdentity, ModelUsage, ToolSpec,
};

/// Current context size, calibrated against a successful provider request when available.
///
/// Calibration is session-local and is not persisted. It applies only while the
/// measured request remains an unchanged prefix with the same provider and tools.
/// Subsequent messages contribute their local estimate, not cumulative run usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ContextEstimate {
    estimated_tokens: u64,
    provider_reported_tokens: Option<u64>,
    provider_request_estimated_tokens: Option<u64>,
    reported_context_window: Option<u64>,
}

impl ContextEstimate {
    /// Builds an uncalibrated estimate, for example before a resumed session's first report.
    pub const fn from_estimated_tokens(estimated_tokens: u64) -> Self {
        Self {
            estimated_tokens,
            provider_reported_tokens: None,
            provider_request_estimated_tokens: None,
            reported_context_window: None,
        }
    }

    /// Provider-neutral estimate of the current messages, tools, and request overhead.
    pub const fn estimated_tokens(self) -> u64 {
        self.estimated_tokens
    }

    /// Latest measured prompt size plus locally estimated append-only additions.
    pub fn tokens(self) -> u64 {
        match (
            self.provider_reported_tokens,
            self.provider_request_estimated_tokens,
        ) {
            (Some(reported), Some(estimated)) => {
                reported.saturating_add(self.estimated_tokens.saturating_sub(estimated))
            }
            _ => self.estimated_tokens,
        }
    }

    /// Inclusive prompt tokens from the last applicable successful request, not a run total.
    pub const fn provider_reported_tokens(self) -> Option<u64> {
        self.provider_reported_tokens
    }

    /// Local estimate of that exact request, including its tool schemas.
    pub const fn provider_request_estimated_tokens(self) -> Option<u64> {
        self.provider_request_estimated_tokens
    }

    pub const fn reported_context_window(self) -> Option<u64> {
        self.reported_context_window
    }

    /// Converts a model-token target into a conservative local-estimator budget.
    ///
    /// Uses the larger of the current and measured request's token ratios, rounds
    /// down, and never enlarges the target. This is a sizing heuristic, not a
    /// tokenizer guarantee for replacement text with a different token distribution.
    pub fn estimated_budget(self, target_tokens: u64) -> u64 {
        let scale = |estimated: u64, measured: u64| {
            if measured <= estimated {
                target_tokens
            } else {
                ((u128::from(target_tokens) * u128::from(estimated)) / u128::from(measured)) as u64
            }
        };
        let current = scale(self.estimated_tokens, self.tokens());
        match (
            self.provider_request_estimated_tokens,
            self.provider_reported_tokens,
        ) {
            (Some(estimated), Some(measured)) => current.min(scale(estimated, measured)),
            _ => current,
        }
    }
}

/// A request anchor contains no conversation copy. Fingerprints validate exact
/// prefix content, including replay metadata and tool schemas, at history boundaries.
#[derive(Clone, Debug)]
struct Baseline {
    identity: ModelIdentity,
    message_count: usize,
    fingerprint: u64,
    estimate: ContextEstimate,
}

#[derive(Clone, Debug)]
pub(crate) struct ContextAccounting {
    baseline: Option<Baseline>,
    current: ContextEstimate,
    current_messages: Option<usize>,
    tools_fingerprint: u64,
}

impl ContextAccounting {
    pub(crate) fn new(history: &[Message], tools: &[ToolSpec]) -> Self {
        Self {
            baseline: None,
            current: ContextEstimate::from_estimated_tokens(estimate_context_tokens(
                history, tools,
            )),
            current_messages: Some(history.len()),
            tools_fingerprint: fingerprint(&[], tools),
        }
    }

    pub(crate) fn current(&self) -> ContextEstimate {
        self.current
    }

    /// SDK working history is append-only between explicit replacements. Reuse
    /// its measured prefix and count only new messages. Host-proposed histories
    /// must use `estimate` instead; equal length does not prove equal content.
    /// Shorter or rewritten working histories must use `replace`.
    pub(crate) fn advance(
        &mut self,
        history: &[Message],
        tools: &[ToolSpec],
        identity: &ModelIdentity,
    ) -> ContextEstimate {
        let tools_fingerprint = fingerprint(&[], tools);
        if tools_fingerprint != self.tools_fingerprint
            || self
                .baseline
                .as_ref()
                .is_some_and(|baseline| baseline.identity != *identity)
        {
            self.invalidate();
        }
        match self.current_messages {
            Some(count) => {
                for message in &history[count..] {
                    self.append(message);
                }
            }
            None => {
                self.publish(self.estimate(history, tools, identity));
                self.current_messages = Some(history.len());
            }
        }
        self.tools_fingerprint = tools_fingerprint;
        self.current
    }

    /// Replacements may alter old messages, including equal-sized ones.
    pub(crate) fn replace(
        &mut self,
        history: &[Message],
        tools: &[ToolSpec],
        identity: &ModelIdentity,
    ) {
        self.publish(self.estimate(history, tools, identity));
        self.current_messages = Some(history.len());
        self.tools_fingerprint = fingerprint(&[], tools);
    }

    pub(crate) fn estimate(
        &self,
        history: &[Message],
        tools: &[ToolSpec],
        identity: &ModelIdentity,
    ) -> ContextEstimate {
        let mut estimate =
            ContextEstimate::from_estimated_tokens(estimate_context_tokens(history, tools));
        if let Some(baseline) = &self.baseline {
            if baseline.identity == *identity
                && history.len() >= baseline.message_count
                && fingerprint(&history[..baseline.message_count], tools) == baseline.fingerprint
            {
                estimate.provider_reported_tokens = baseline.estimate.provider_reported_tokens;
                estimate.provider_request_estimated_tokens =
                    baseline.estimate.provider_request_estimated_tokens;
                estimate.reported_context_window = baseline.estimate.reported_context_window;
            }
        }
        estimate
    }

    fn publish(&mut self, estimate: ContextEstimate) {
        // A mismatched prefix must not become applicable again after a later edit.
        if estimate.provider_reported_tokens.is_none() {
            self.baseline = None;
        }
        self.current = estimate;
    }

    pub(crate) fn record(
        &mut self,
        history: &[Message],
        tools: &[ToolSpec],
        identity: ModelIdentity,
        usage: &ModelUsage,
        request_estimate: ContextEstimate,
    ) {
        let Some(reported) = usage.inclusive_prompt_tokens() else {
            return;
        };
        let estimate = ContextEstimate {
            estimated_tokens: request_estimate.estimated_tokens(),
            provider_reported_tokens: Some(reported),
            provider_request_estimated_tokens: Some(request_estimate.estimated_tokens()),
            reported_context_window: usage.context_window,
        };
        self.baseline = Some(Baseline {
            identity,
            message_count: history.len(),
            fingerprint: fingerprint(history, tools),
            estimate,
        });
        self.current = estimate;
        self.current_messages = Some(history.len());
        self.tools_fingerprint = fingerprint(&[], tools);
    }

    pub(crate) fn invalidate(&mut self) {
        self.baseline = None;
        self.current = ContextEstimate::from_estimated_tokens(self.current.estimated_tokens);
        self.current_messages = None;
    }

    pub(crate) fn append(&mut self, message: &Message) {
        self.current_messages = self.current_messages.map(|count| count + 1);
        self.current.estimated_tokens = self
            .current
            .estimated_tokens
            .saturating_add(crate::model::context::estimate_message_tokens(message));
    }
}

fn fingerprint(history: &[Message], tools: &[ToolSpec]) -> u64 {
    struct Sink(DefaultHasher);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.write(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Sink(DefaultHasher::new());
    serde_json::to_writer(&mut sink, &(history, tools))
        .expect("provider messages and tool specifications are serializable");
    sink.0.finish()
}

#[cfg(test)]
#[path = "context_estimate_tests.rs"]
mod tests;
