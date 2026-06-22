use crate::model::{estimate_context_usage, ContextUsage, Message, ModelUsage};
use crate::tool::ToolSpec;

#[derive(Debug, Default)]
pub(super) struct ContextTracker {
    configured_context_window: Option<u64>,
    last_context_window: Option<u64>,
    unknown_after_compaction: bool,
}

impl ContextTracker {
    pub(super) fn set_configured_window(&mut self, context_window: Option<u64>) {
        self.configured_context_window = context_window.filter(|window| *window > 0);
    }

    pub(super) fn replace_provider(&mut self) {
        self.last_context_window = None;
        self.unknown_after_compaction = false;
    }

    pub(super) fn reset(&mut self) {
        self.last_context_window = None;
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
        estimate_context_usage(messages, specs, self.context_window())
    }

    pub(super) fn record_provider_usage(&mut self, usage: &ModelUsage) -> Option<ContextUsage> {
        if let Some(context_window) = usage.context_window {
            self.last_context_window = Some(context_window);
        }
        let context_usage = ContextUsage::from_model_usage(usage)?;
        self.unknown_after_compaction = false;
        Some(context_usage)
    }

    pub(super) fn record_compaction(&mut self) -> ContextUsage {
        self.unknown_after_compaction = true;
        ContextUsage::unknown_after_compaction(self.context_window())
    }

    pub(super) fn context_window(&self) -> Option<u64> {
        self.last_context_window.or(self.configured_context_window)
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
    fn provider_reported_usage_replaces_estimate_and_window() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));

        let usage = tracker
            .record_provider_usage(&ModelUsage {
                input_tokens: Some(100),
                cache_read_tokens: Some(50),
                context_window: Some(8_000),
                ..ModelUsage::default()
            })
            .unwrap();

        assert_eq!(usage.source, ContextUsageSource::ProviderReported);
        assert_eq!(usage.tokens, Some(150));
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

        tracker.record_provider_usage(&ModelUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            ..ModelUsage::default()
        });

        assert!(tracker
            .before_provider_request(
                &[Message::User(vec![ContentBlock::Text("after".into())])],
                &[]
            )
            .is_some());
    }

    #[test]
    fn reset_clears_provider_window_and_unknown_state() {
        let mut tracker = ContextTracker::default();
        tracker.set_configured_window(Some(4_000));
        tracker.record_provider_usage(&ModelUsage {
            input_tokens: Some(10),
            context_window: Some(8_000),
            ..ModelUsage::default()
        });
        tracker.record_compaction();

        tracker.reset();

        assert_eq!(tracker.context_window(), Some(4_000));
        assert!(tracker
            .before_provider_request(&[Message::user_text("after")], &[])
            .is_some());
    }
}
