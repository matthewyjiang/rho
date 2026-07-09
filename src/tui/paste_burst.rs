use std::time::{Duration, Instant};

const PASTE_BURST_GAP: Duration = Duration::from_millis(12);
const PASTE_ENTER_SUPPRESSION: Duration = Duration::from_millis(120);
const PASTE_BURST_MIN_CHARS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PasteBurstEnter {
    Buffered,
    InsertNewline,
    NotPaste,
}

#[derive(Default)]
pub(super) struct PasteBurst {
    pending_text: String,
    last_event_at: Option<Instant>,
    plain_char_count: usize,
    suppress_enter_until: Option<Instant>,
}

impl PasteBurst {
    pub(super) fn has_pending(&self) -> bool {
        !self.pending_text.is_empty()
    }

    pub(super) fn can_continue(&self, now: Instant) -> bool {
        if !self.has_pending() {
            return true;
        }

        self.last_event_at
            .is_some_and(|last| now.saturating_duration_since(last) <= PASTE_BURST_GAP)
    }

    pub(super) fn push_plain_char(&mut self, ch: char, now: Instant) {
        if !self.has_pending() {
            self.plain_char_count = 0;
            self.suppress_enter_until = None;
        }

        self.pending_text.push(ch);
        self.last_event_at = Some(now);
        self.plain_char_count = self.plain_char_count.saturating_add(1);
        if self.plain_char_count >= PASTE_BURST_MIN_CHARS {
            self.suppress_enter_until = now.checked_add(PASTE_ENTER_SUPPRESSION);
        }
    }

    pub(super) fn push_enter_if_paste(&mut self, now: Instant) -> PasteBurstEnter {
        let follows_pending_burst = self
            .last_event_at
            .is_some_and(|last| now.saturating_duration_since(last) <= PASTE_BURST_GAP)
            && self.plain_char_count >= PASTE_BURST_MIN_CHARS;
        let follows_plain_text_burst = self.suppresses_enter_at(now);
        if !follows_pending_burst && !follows_plain_text_burst {
            return PasteBurstEnter::NotPaste;
        }

        self.suppress_enter_until = now.checked_add(PASTE_ENTER_SUPPRESSION);
        if self.has_pending() {
            self.pending_text.push('\n');
            self.last_event_at = Some(now);
            PasteBurstEnter::Buffered
        } else {
            PasteBurstEnter::InsertNewline
        }
    }

    pub(super) fn is_due(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| now >= deadline)
    }

    pub(super) fn poll_timeout(&self, now: Instant, idle_timeout: Duration) -> Duration {
        let Some(deadline) = self.deadline() else {
            return idle_timeout;
        };

        deadline
            .checked_duration_since(now)
            .unwrap_or_default()
            .min(idle_timeout)
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.last_event_at
            .and_then(|last| last.checked_add(PASTE_BURST_GAP))
    }

    pub(super) fn take_pending(&mut self) -> Option<String> {
        if self.pending_text.is_empty() {
            self.clear_pending_text();
            return None;
        }

        let text = std::mem::take(&mut self.pending_text);
        self.clear_pending_text();
        Some(text)
    }

    pub(super) fn clear(&mut self) {
        self.pending_text.clear();
        self.last_event_at = None;
        self.plain_char_count = 0;
        self.suppress_enter_until = None;
    }

    fn clear_pending_text(&mut self) {
        self.pending_text.clear();
        self.last_event_at = None;
        self.plain_char_count = 0;
    }

    fn suppresses_enter_at(&self, now: Instant) -> bool {
        self.suppress_enter_until
            .is_some_and(|deadline| now <= deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_char_enter_is_not_buffered_as_paste() {
        let start = Instant::now();
        let mut burst = PasteBurst::default();

        burst.push_plain_char('y', start);

        assert_eq!(
            burst.push_enter_if_paste(start + Duration::from_millis(1)),
            PasteBurstEnter::NotPaste
        );
        assert_eq!(burst.take_pending().as_deref(), Some("y"));
    }

    #[test]
    fn enter_after_idle_gap_is_not_part_of_paste() {
        let start = Instant::now();
        let mut burst = PasteBurst::default();

        burst.push_plain_char('a', start);

        assert_eq!(
            burst.push_enter_if_paste(start + PASTE_BURST_GAP + Duration::from_millis(1)),
            PasteBurstEnter::NotPaste
        );
        assert_eq!(burst.take_pending().as_deref(), Some("a"));
    }

    #[test]
    fn rapid_plain_text_burst_extends_enter_suppression() {
        let start = Instant::now();
        let mut burst = PasteBurst::default();

        burst.push_plain_char('a', start);
        burst.push_plain_char('b', start + Duration::from_millis(1));

        assert_eq!(
            burst.push_enter_if_paste(start + Duration::from_millis(50)),
            PasteBurstEnter::Buffered
        );
        assert_eq!(burst.take_pending().as_deref(), Some("ab\n"));
    }

    #[test]
    fn enter_suppression_survives_literal_flush() {
        let start = Instant::now();
        let mut burst = PasteBurst::default();

        burst.push_plain_char('a', start);
        burst.push_plain_char('b', start + Duration::from_millis(1));
        assert_eq!(burst.take_pending().as_deref(), Some("ab"));

        assert_eq!(
            burst.push_enter_if_paste(start + Duration::from_millis(50)),
            PasteBurstEnter::InsertNewline
        );
    }
}
