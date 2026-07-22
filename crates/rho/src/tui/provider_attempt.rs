use super::Entry;

/// Tracks the transcript boundary for the current provider attempt.
///
/// Provider retries replace assistant and reasoning output from the failed
/// attempt while preserving notices and completed tool entries.
#[derive(Default)]
pub(super) struct ProviderAttempt {
    start: Option<usize>,
}

impl ProviderAttempt {
    pub(super) fn begin(&mut self, transcript_len: usize) {
        self.start = Some(transcript_len);
    }

    pub(super) fn can_append_to_last(&self, transcript_len: usize) -> bool {
        self.start.is_none_or(|start| start < transcript_len)
    }

    /// Removes replaceable output from the current attempt and advances the
    /// boundary to the end of the retained transcript.
    ///
    /// Returns the first potentially changed entry for cache invalidation.
    pub(super) fn reset_output(&mut self, transcript: &mut Vec<Entry>) -> Option<usize> {
        let start = self.start?;
        let original_len = transcript.len();
        let mut index = 0;
        transcript.retain(|entry| {
            let keep = index < start || !entry.is_provider_replaceable();
            index += 1;
            keep
        });
        self.start = Some(transcript.len());
        (transcript.len() != original_len).then_some(start)
    }
}

#[cfg(test)]
#[path = "provider_attempt_tests.rs"]
mod tests;
