use std::{
    num::NonZeroU64,
    time::{Duration, Instant},
};

use super::QuestionnaireComposer;

/// Once touched, a form stays paused until explicit submit or cancel. There is
/// no idle-resume timer that could submit a user's partially edited answers.
#[derive(Debug)]
pub(super) enum QuestionnaireTimer {
    Running {
        opened: Instant,
        duration: Duration,
        at_deadline: AtDeadline,
    },
    Paused,
}

/// Who answers once a running timer reaches its deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AtDeadline {
    /// The form submits the request's fallback itself.
    SubmitFallback,
    /// Someone else does, as the daemon answers a daemon session's question
    /// itself: the form only counts down, since an answer it sent would
    /// count as the user's own.
    OtherAnswers,
}

impl QuestionnaireComposer {
    pub(in crate::tui) fn start_timeout(&mut self, seconds: Option<NonZeroU64>, now: Instant) {
        self.timeout = seconds
            .filter(|_| self.request.timeout_fallback().is_some())
            .map(|seconds| QuestionnaireTimer::Running {
                opened: now,
                duration: Duration::from_secs(seconds.get()),
                at_deadline: AtDeadline::SubmitFallback,
            });
    }

    /// Counts down `remaining` until someone else answers with the
    /// request's fallback, replacing any timer of the form's own. Any
    /// interaction pauses it, as it pauses a timer of the form's own.
    pub(in crate::tui) fn count_down_to_other_answer(&mut self, remaining: Duration, now: Instant) {
        self.timeout = Some(QuestionnaireTimer::Running {
            opened: now,
            duration: remaining,
            at_deadline: AtDeadline::OtherAnswers,
        });
    }

    pub(in crate::tui) fn observe_input(&mut self, event: &crossterm::event::Event) {
        use crossterm::event::Event;
        if matches!(event, Event::Key(_) | Event::Paste(_) | Event::Mouse(_)) {
            self.pause_timeout();
        }
    }

    /// Deferred paste can edit a newly opened form without another terminal
    /// event. Keep the edit guard with the composer, not in shared paste helpers.
    pub(in crate::tui) fn pause_timeout(&mut self) {
        if self.timeout.is_some() {
            self.timeout = Some(QuestionnaireTimer::Paused);
        }
    }

    pub(in crate::tui) fn timeout_running(&self) -> bool {
        matches!(self.timeout, Some(QuestionnaireTimer::Running { .. }))
    }

    pub(super) fn timeout_notice(&self, now: Instant) -> Option<String> {
        match self.timeout.as_ref()? {
            QuestionnaireTimer::Running {
                opened, duration, ..
            } => {
                let remaining = duration.saturating_sub(now.saturating_duration_since(*opened));
                let seconds = remaining
                    .as_secs()
                    .saturating_add(u64::from(remaining.subsec_nanos() > 0));
                Some(format!("Fallback in {seconds}s; any interaction pauses"))
            }
            QuestionnaireTimer::Paused => {
                Some("Fallback paused; submit or cancel to continue".into())
            }
        }
    }

    pub(in crate::tui) fn submit_timeout_if_due(&mut self, now: Instant) -> Option<String> {
        let Some(QuestionnaireTimer::Running {
            opened,
            duration,
            at_deadline: AtDeadline::SubmitFallback,
        }) = &self.timeout
        else {
            return None;
        };
        if now.saturating_duration_since(*opened) < *duration {
            return None;
        }
        let fallback = self.request.timeout_fallback()?.clone();
        let display = format!(
            "timeout fallback: {}",
            self.request.timeout_reason().unwrap_or_default()
        );
        self.response.send_response(fallback);
        self.timeout = None;
        Some(display)
    }
}

#[cfg(test)]
#[path = "timeout_tests.rs"]
mod tests;
