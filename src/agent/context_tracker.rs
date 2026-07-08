use crate::model::{estimate_context_usage, ContextUsage, Message, ModelUsage};
use crate::tool::ToolSpec;

#[derive(Debug, Default)]
pub(super) struct ContextTracker {
    configured_context_window: Option<u64>,
    last_context_window: Option<u64>,
    reported_anchor: Option<ReportedAnchor>,
    unknown_after_compaction: bool,
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

    pub(super) fn before_provider_request(
        &self,
        messages: &[Message],
        specs: &[ToolSpec],
    ) -> Option<ContextUsage> {
        if self.unknown_after_compaction {
            None
        } else {
            Some(estimate_context_usage(
                messages,
                specs,
                self.context_window(),
            ))
        }
    }

    pub(super) fn estimate_for_compaction(
        &self,
        messages: &[Message],
        specs: &[ToolSpec],
    ) -> ContextUsage {
        let mut usage = estimate_context_usage(messages, specs, self.context_window());
        if let (Some(anchor), Some(estimated)) = (self.reported_anchor, usage.tokens) {
            usage.tokens = Some(
                anchor
                    .reported_tokens
                    .saturating_add(estimated.saturating_sub(anchor.estimated_tokens)),
            );
        }
        usage
    }

    pub(super) fn record_provider_usage(
        &mut self,
        usage: &ModelUsage,
        estimated_request_tokens: u64,
    ) -> Option<ContextUsage> {
        if let Some(context_window) = usage.context_window {
            self.last_context_window = Some(context_window);
        }
        if let Some(reported_tokens) = usage.total_input_tokens() {
            self.reported_anchor = Some(ReportedAnchor {
                reported_tokens,
                estimated_tokens: estimated_request_tokens,
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

    #[test]
    fn estimated_usage_before_provider_request_uses_configured_window() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));

        let usage = tracker
            .before_provider_request(&[Message::user_text("hello")], &[])
            .unwrap();

        assert_eq!(usage.source, ContextUsageSource::Estimated);
        assert_eq!(usage.context_window, Some(4_000));
        assert!(usage.tokens.unwrap() > 0);
    }

    #[test]
    fn provider_reported_usage_keeps_safer_configured_window() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));

        let usage = tracker
            .record_provider_usage(
                &ModelUsage {
                    input_tokens: Some(100),
                    cache_read_tokens: Some(50),
                    context_window: Some(8_000),
                    ..ModelUsage::default()
                },
                0,
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

        let usage = tracker
            .record_provider_usage(
                &ModelUsage {
                    input_tokens: Some(100),
                    context_window: Some(8_000),
                    ..ModelUsage::default()
                },
                0,
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
        assert_eq!(
            tracker.before_provider_request(&[Message::user_text("after")], &[]),
            None
        );
    }

    #[test]
    fn provider_usage_clears_unknown_after_compaction() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));
        tracker.record_compaction();

        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                ..ModelUsage::default()
            },
            0,
        );

        assert!(tracker
            .before_provider_request(
                &[Message::User(vec![ContentBlock::Text("after".into())])],
                &[]
            )
            .is_some());
    }

    #[test]
    fn compaction_estimate_anchors_to_provider_reported_usage() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(10_000));
        let messages = vec![Message::user_text("hello")];
        let estimate_at_report = tracker
            .estimate_for_compaction(&messages, &[])
            .tokens
            .unwrap();

        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(estimate_at_report + 500),
                ..ModelUsage::default()
            },
            estimate_at_report,
        );

        let mut grown = messages.clone();
        grown.push(Message::assistant_text("a longer assistant reply"));
        let local_estimate = estimate_context_usage(&grown, &[], None).tokens.unwrap();
        let anchored = tracker.estimate_for_compaction(&grown, &[]).tokens.unwrap();
        assert_eq!(
            anchored,
            estimate_at_report + 500 + (local_estimate - estimate_at_report)
        );
    }

    #[test]
    fn compaction_clears_reported_anchor() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(10_000));
        let messages = vec![Message::user_text("hello")];
        let local_estimate = tracker
            .estimate_for_compaction(&messages, &[])
            .tokens
            .unwrap();
        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(local_estimate + 500),
                ..ModelUsage::default()
            },
            local_estimate,
        );

        tracker.record_compaction();

        assert_eq!(
            tracker.estimate_for_compaction(&messages, &[]).tokens,
            Some(local_estimate)
        );
    }

    #[test]
    fn reset_clears_provider_window_and_unknown_state() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));
        tracker.record_provider_usage(
            &ModelUsage {
                input_tokens: Some(10),
                context_window: Some(8_000),
                ..ModelUsage::default()
            },
            0,
        );
        tracker.record_compaction();

        tracker.reset();

        assert_eq!(tracker.context_window(), Some(4_000));
        assert!(tracker
            .before_provider_request(&[Message::user_text("after")], &[])
            .is_some());
    }
}
