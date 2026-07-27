//! Paces streamed text onto the screen.
//!
//! Providers flush their socket in bursts. Measured against grok-4.5, word-sized
//! deltas of about five characters arrive two or three at a time, roughly every
//! 57ms, and occasionally stall for far longer. Drawing each burst as it lands
//! shows the network's timing rather than the model's.
//!
//! The pacer keeps a short reserve of text in hand and releases it at the rate
//! the model is actually producing. The reserve covers a late flush so text
//! keeps moving through it, and the measured rate keeps the reserve from either
//! draining empty or trailing further and further behind.

use std::time::{Duration, Instant};

/// Text kept in reserve, measured as how long it takes to play out.
///
/// This is the stall the pacer can absorb, and the lag it adds in exchange.
const TARGET_LEAD: Duration = Duration::from_millis(100);

/// How quickly a reserve that is too big or too small is steered back to
/// [`TARGET_LEAD`].
///
/// Short enough to correct within a sentence, long enough that a single late
/// flush does not visibly change the reading speed.
const LEAD_CORRECTION: Duration = Duration::from_millis(500);

/// Release rate used until arrivals have been observed for [`MIN_RATE_SAMPLE`].
///
/// Close to the rate measured from grok-4.5, so the opening words of a stream
/// are paced sensibly before there is anything to measure.
const INITIAL_CHARS_PER_SEC: f64 = 240.0;

/// Arrival window needed before the measured rate replaces the initial guess.
const MIN_RATE_SAMPLE: Duration = Duration::from_millis(250);

/// Reserve above which text is released without pacing.
///
/// A response that arrives whole, from a provider that does not stream or from
/// a replayed transcript, must appear at once rather than type itself out.
const MAX_RESERVE_CHARS: usize = 2048;

/// Interval between opportunities to release reserved text.
pub(super) const STREAM_PACE_INTERVAL: Duration = Duration::from_millis(24);

/// Releases streamed text at the model's measured speed.
#[derive(Debug)]
pub(super) struct StreamPacer {
    /// Start of the arrival window backing the rate estimate.
    first_arrival: Option<Instant>,
    chars_arrived: u64,
    last_release: Option<Instant>,
    /// Fraction of a character carried into the next release.
    carry: f64,
}

impl Default for StreamPacer {
    fn default() -> Self {
        Self {
            first_arrival: None,
            chars_arrived: 0,
            last_release: None,
            carry: 0.0,
        }
    }
}

impl StreamPacer {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Records text arriving from the provider, for the rate estimate.
    ///
    /// `reserve_was_empty` starts a fresh playback interval. Time spent with no
    /// text available must not be charged against a later burst.
    pub(super) fn record_arrival(&mut self, now: Instant, chars: usize, reserve_was_empty: bool) {
        if reserve_was_empty {
            self.first_arrival = Some(now);
            self.chars_arrived = chars as u64;
            self.last_release = Some(now);
            self.carry = 0.0;
        } else {
            self.first_arrival.get_or_insert(now);
            self.chars_arrived = self.chars_arrived.saturating_add(chars as u64);
        }
    }

    /// Characters the screen may take now, given how many are held back.
    pub(super) fn release_allowance(&mut self, now: Instant, reserve_chars: usize) -> usize {
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

        let rate = self.arrival_rate(now);
        let lead = rate * TARGET_LEAD.as_secs_f64();
        // Steer the reserve toward the target lead: a reserve above it releases
        // faster, one below it releases slower, so playback tracks the model.
        let correction = (reserve_chars as f64 - lead) / LEAD_CORRECTION.as_secs_f64();
        let released = (rate + correction).max(0.0) * elapsed + self.carry;
        let whole = released.floor();
        self.carry = released - whole;
        (whole as usize).min(reserve_chars)
    }

    /// Characters per second the provider has been producing.
    fn arrival_rate(&self, now: Instant) -> f64 {
        let Some(first_arrival) = self.first_arrival else {
            return INITIAL_CHARS_PER_SEC;
        };
        let window = now.saturating_duration_since(first_arrival);
        if window < MIN_RATE_SAMPLE {
            return INITIAL_CHARS_PER_SEC;
        }
        let rate = self.chars_arrived as f64 / window.as_secs_f64();
        if rate > 0.0 {
            rate
        } else {
            INITIAL_CHARS_PER_SEC
        }
    }
}

#[cfg(test)]
#[path = "stream_pace_tests.rs"]
mod tests;
