//! Paces streamed text onto the screen.
//!
//! Providers flush their socket in bursts. Measured against grok-4.5, word-sized
//! deltas of about five characters arrive two or three at a time, roughly every
//! 57ms, and occasionally stall for far longer. Drawing each burst as it lands
//! shows the network's timing rather than the model's.
//!
//! Held text is drained over [`TARGET_LEAD`]. That keeps about a lead's worth of
//! characters in reserve at steady state, which covers a late flush, and plays
//! text at the arrival rate without measuring it.

use std::time::{Duration, Instant};

use super::{
    markdown::CodeFenceState, stream::AppendOnlyStream, StreamKind, StreamUi,
    STREAM_PREVIEW_MIN_CHARS, STREAM_UI_TICK,
};

/// Text kept in reserve, measured as how long it takes to play out.
///
/// This is the stall the pacer can absorb, and the lag it adds in exchange.
const TARGET_LEAD: Duration = Duration::from_millis(100);

/// Reserve above which text is released without pacing.
///
/// A response that arrives whole, from a provider that does not stream or from
/// a replayed transcript, must appear at once rather than type itself out.
const MAX_RESERVE_CHARS: usize = 2048;

/// Releases held text by draining the reserve over [`TARGET_LEAD`].
#[derive(Debug, Default)]
pub(super) struct StreamPacer {
    last_release: Option<Instant>,
    /// Fraction of a character carried into the next release.
    carry: f64,
}

impl StreamPacer {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Starts a fresh playback interval after the reserve was empty.
    ///
    /// Time spent with no text available must not be charged against a later burst.
    fn note_refill(&mut self, now: Instant) {
        self.last_release = Some(now);
        self.carry = 0.0;
    }

    /// Characters the screen may take now, given how many are held back.
    fn release_allowance(&mut self, now: Instant, reserve_chars: usize) -> usize {
        if reserve_chars == 0 {
            self.last_release = Some(now);
            self.carry = 0.0;
            return 0;
        }
        if reserve_chars > MAX_RESERVE_CHARS {
            self.last_release = Some(now);
            self.carry = 0.0;
            return reserve_chars;
        }
        let Some(last_release) = self.last_release else {
            // Nothing has been released yet, so there is no interval to bill
            // against. Start the clock and let the next tick move text.
            self.last_release = Some(now);
            return 0;
        };
        let elapsed = now.saturating_duration_since(last_release).as_secs_f64();
        if elapsed <= 0.0 {
            return 0;
        }
        self.last_release = Some(now);

        // Drain the reserve over TARGET_LEAD. At steady arrival rate R the
        // reserve settles near R * TARGET_LEAD, so release rate matches R.
        let released = reserve_chars as f64 * elapsed / TARGET_LEAD.as_secs_f64() + self.carry;
        let whole = released.floor();
        self.carry = released - whole;
        (whole as usize).min(reserve_chars)
    }
}

impl StreamUi {
    pub(super) fn stream(&self, kind: StreamKind) -> &AppendOnlyStream {
        match kind {
            StreamKind::Assistant => &self.assistant_stream,
            StreamKind::Reasoning => &self.reasoning_stream,
        }
    }

    pub(super) fn stream_mut(&mut self, kind: StreamKind) -> &mut AppendOnlyStream {
        match kind {
            StreamKind::Assistant => &mut self.assistant_stream,
            StreamKind::Reasoning => &mut self.reasoning_stream,
        }
    }

    pub(super) fn code_fence(&self, kind: StreamKind) -> &CodeFenceState {
        match kind {
            StreamKind::Assistant => &self.assistant_stream_code_fence,
            StreamKind::Reasoning => &self.reasoning_stream_code_fence,
        }
    }

    pub(super) fn code_fence_mut(&mut self, kind: StreamKind) -> &mut CodeFenceState {
        self.bump_fence_generation();
        match kind {
            StreamKind::Assistant => &mut self.assistant_stream_code_fence,
            StreamKind::Reasoning => &mut self.reasoning_stream_code_fence,
        }
    }

    /// Appends provider text into the hold and releases what the pacer allows.
    pub(super) fn push_delta(&mut self, kind: StreamKind, text: &str, now: Instant) {
        if text.is_empty() {
            self.schedule_tick(kind, now);
            return;
        }
        let was_empty = self.hold.is_empty();
        self.hold.push_str(text);
        if was_empty {
            self.pacer.note_refill(now);
        }
        self.release_into(kind, now);
        self.schedule_tick(kind, now);
    }

    /// Advances a due stream UI tick: release held text, then leave preview to
    /// the caller.
    ///
    /// Returns whether any held text was moved into the stream.
    pub(super) fn on_tick(&mut self, now: Instant) -> bool {
        if self
            .stream_tick_deadline
            .is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        self.stream_tick_deadline = None;
        let Some(kind) = self.current_stream_kind else {
            return false;
        };
        let released = self.release_into(kind, now);
        // Keep the cadence only while held text remains. A one-shot tick from
        // schedule_tick for a static partial preview must not reschedule itself.
        if !self.hold.is_empty() {
            self.schedule_tick(kind, now);
        }
        released
    }

    /// Dumps every held character into `kind` without pacing.
    pub(super) fn flush_hold(&mut self, kind: StreamKind) {
        if self.hold.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.hold);
        self.stream_mut(kind).push_delta(&text);
        self.pacer.reset();
    }

    /// Drops held text without showing it.
    pub(super) fn discard_hold(&mut self) {
        self.hold.clear();
        self.pacer.reset();
    }

    pub(super) fn schedule_tick(&mut self, kind: StreamKind, now: Instant) {
        let pending_chars = self.stream(kind).pending_text().chars().count();
        let needs_tick = !self.hold.is_empty() || pending_chars >= STREAM_PREVIEW_MIN_CHARS;
        if !needs_tick {
            self.stream_tick_deadline = None;
        } else if self.stream_tick_deadline.is_none() {
            self.stream_tick_deadline = Some(now + STREAM_UI_TICK);
        }
    }

    pub(super) fn clear_tick_deadline(&mut self) {
        self.stream_tick_deadline = None;
    }

    /// Moves up to the pacer's allowance from the hold into the stream.
    ///
    /// Returns whether any text was released.
    fn release_into(&mut self, kind: StreamKind, now: Instant) -> bool {
        let reserve = self.hold.chars().count();
        let chars = self.pacer.release_allowance(now, reserve);
        if chars == 0 {
            return false;
        }
        let byte_end = self
            .hold
            .char_indices()
            .nth(chars)
            .map_or(self.hold.len(), |(byte_index, _)| byte_index);
        let released: String = self.hold.drain(..byte_end).collect();
        self.stream_mut(kind).push_delta(&released);
        true
    }
}

#[cfg(test)]
#[path = "stream_pace_tests.rs"]
mod tests;
