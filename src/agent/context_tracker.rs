use crate::model::{context::estimate_context_tokens, ContextUsage, Message, ModelUsage};
use crate::tool::ToolSpec;

#[derive(Debug, Default)]
pub(super) struct ContextTracker {
    configured_context_window: Option<u64>,
    last_context_window: Option<u64>,
    reported_anchor: Option<ReportedAnchor>,
    unknown_after_compaction: bool,
}

/// The local token estimate for one exact message and tool snapshot.
///
/// Keeping this value explicit lets compaction checks, context events, and
/// provider-usage anchoring share one full-history estimation pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RequestContextEstimate {
    tokens: u64,
}

/// Pairs a provider-reported input total with the local estimate for the same
/// request, so later estimates can be corrected by the observed difference.
#[derive(Clone, Copy, Debug)]
struct ReportedAnchor {
    reported_tokens: u64,
    estimated_tokens: u64,
}

impl ContextTracker {
    pub(super) fn set_configured_window(&mut self, context_window: Option<u64>) {
        self.configured_context_window = context_window.filter(|window| *window > 0);
    }

    pub(super) fn replace_provider(&mut self) {
        self.last_context_window = None;
        self.reported_anchor = None;
        self.unknown_after_compaction = false;
    }

    pub(super) fn reset(&mut self) {
        self.last_context_window = None;
        self.reported_anchor = None;
        self.unknown_after_compaction = false;
    }

    pub(super) fn history_replaced(&mut self) {
        self.reported_anchor = None;
        self.unknown_after_compaction = false;
    }

    pub(super) fn estimate_request(
        &self,
        messages: &[Message],
        specs: &[ToolSpec],
    ) -> RequestContextEstimate {
        RequestContextEstimate {
            tokens: estimate_context_tokens(messages, specs),
        }
    }

    pub(super) fn before_provider_request(
        &self,
        estimate: RequestContextEstimate,
    ) -> Option<ContextUsage> {
        if self.unknown_after_compaction {
            None
        } else {
            Some(ContextUsage::estimated(
                estimate.tokens,
                self.context_window(),
            ))
        }
    }

    pub(super) fn estimate_for_compaction(&self, estimate: RequestContextEstimate) -> ContextUsage {
        let tokens = if let Some(anchor) = self.reported_anchor {
            anchor
                .reported_tokens
                .saturating_add(estimate.tokens.saturating_sub(anchor.estimated_tokens))
        } else {
            estimate.tokens
        };
        ContextUsage::estimated(tokens, self.context_window())
    }

    pub(super) fn record_provider_usage(
        &mut self,
        usage: &ModelUsage,
        estimate: RequestContextEstimate,
    ) -> Option<ContextUsage> {
        if let Some(context_window) = usage.context_window {
            self.last_context_window = Some(context_window);
        }
        if let Some(reported_tokens) = usage.total_input_tokens() {
            self.reported_anchor = Some(ReportedAnchor {
                reported_tokens,
                estimated_tokens: estimate.tokens,
            });
        }
        let mut context_usage = ContextUsage::from_model_usage(usage)?;
        context_usage.context_window = self.context_window();
        self.unknown_after_compaction = false;
        Some(context_usage)
    }

    pub(super) fn record_compaction(&mut self) -> ContextUsage {
        self.reported_anchor = None;
        self.unknown_after_compaction = true;
        ContextUsage::unknown_after_compaction(self.context_window())
    }

    pub(super) fn context_window(&self) -> Option<u64> {
        match (self.last_context_window, self.configured_context_window) {
            (Some(reported), Some(configured)) => Some(reported.min(configured)),
            (Some(reported), None) => Some(reported),
            (None, Some(configured)) => Some(configured),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContentBlock, ContextUsageSource, Message, ModelUsage};

    fn estimate(tracker: &ContextTracker, messages: &[Message]) -> RequestContextEstimate {
        tracker.estimate_request(messages, &[])
    }

    #[test]
    fn one_request_estimate_drives_context_event_and_compaction_check() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));
        let estimate = estimate(&tracker, &[Message::user_text("hello")]);

        let before_request = tracker.before_provider_request(estimate).unwrap();
        let for_compaction = tracker.estimate_for_compaction(estimate);

        assert_eq!(before_request, for_compaction);
        assert_eq!(before_request.source, ContextUsageSource::Estimated);
        assert_eq!(before_request.context_window, Some(4_000));
        assert!(before_request.tokens.unwrap() > 0);
    }

    #[test]
    fn provider_reported_usage_keeps_safer_configured_window() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));
        let estimate = estimate(&tracker, &[]);

        let usage = tracker
            .record_provider_usage(
                &ModelUsage {
                    input_tokens: Some(100),
                    cache_read_tokens: Some(50),
                    context_window: Some(8_000),
                    ..ModelUsage::default()
                },
                estimate,
            )
            .unwrap();

        assert_eq!(usage.source, ContextUsageSource::ProviderReported);
        assert_eq!(usage.tokens, Some(150));
        assert_eq!(usage.context_window, Some(4_000));
        assert_eq!(tracker.context_window(), Some(4_000));
    }

    #[test]
    fn provider_reported_usage_can_lower_configured_window() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(10_000));
        let estimate = estimate(&tracker, &[]);

        let usage = tracker
            .record_provider_usage(
                &ModelUsage {
                    input_tokens: Some(100),
                    context_window: Some(8_000),
                    ..ModelUsage::default()
                },
                estimate,
            )
            .unwrap();

        assert_eq!(usage.context_window, Some(8_000));
        assert_eq!(tracker.context_window(), Some(8_000));
    }

    #[test]
    fn unknown_after_compaction_is_not_overwritten_by_later_estimate() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));

        let usage = tracker.record_compaction();

        assert_eq!(usage.source, ContextUsageSource::UnknownAfterCompaction);
        assert_eq!(usage.context_window, Some(4_000));
        let estimate = estimate(&tracker, &[Message::user_text("after")]);
        assert_eq!(tracker.before_provider_request(estimate), None);
    }

    #[test]
    fn provider_usage_clears_unknown_after_compaction() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));
        tracker.record_compaction();
        let messages = [Message::User(vec![ContentBlock::Text("after".into())])];
        let estimate = estimate(&tracker, &messages);

        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                ..ModelUsage::default()
            },
            estimate,
        );

        assert!(tracker.before_provider_request(estimate).is_some());
    }

    #[test]
    fn compaction_estimate_anchors_to_provider_reported_usage() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(10_000));
        let messages = vec![Message::user_text("hello")];
        let estimate_at_report = estimate(&tracker, &messages);
        let estimated_tokens = tracker
            .estimate_for_compaction(estimate_at_report)
            .tokens
            .unwrap();

        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(estimated_tokens + 500),
                ..ModelUsage::default()
            },
            estimate_at_report,
        );

        let mut grown = messages.clone();
        grown.push(Message::assistant_text("a longer assistant reply"));
        let grown_estimate = estimate(&tracker, &grown);
        let anchored = tracker
            .estimate_for_compaction(grown_estimate)
            .tokens
            .unwrap();
        assert_eq!(
            anchored,
            estimated_tokens + 500 + (grown_estimate.tokens - estimated_tokens)
        );
    }

    #[test]
    fn compaction_clears_reported_anchor() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(10_000));
        let messages = vec![Message::user_text("hello")];
        let estimate = estimate(&tracker, &messages);
        let local_tokens = estimate.tokens;
        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(local_tokens + 500),
                ..ModelUsage::default()
            },
            estimate,
        );

        tracker.record_compaction();

        assert_eq!(
            tracker.estimate_for_compaction(estimate).tokens,
            Some(local_tokens)
        );
    }

    #[test]
    fn reset_clears_provider_window_and_unknown_state() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));
        let initial_estimate = estimate(&tracker, &[]);
        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(10),
                context_window: Some(8_000),
                ..ModelUsage::default()
            },
            initial_estimate,
        );
        tracker.record_compaction();

        tracker.reset();

        assert_eq!(tracker.context_window(), Some(4_000));
        let estimate = estimate(&tracker, &[Message::user_text("after")]);
        assert!(tracker.before_provider_request(estimate).is_some());
    }
}
