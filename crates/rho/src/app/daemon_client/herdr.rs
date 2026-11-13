//! A daemon session's job state as the Herdr pane state of the client
//! that shows it.

use rho_protocol::{JobState, SessionId};

use crate::herdr::{HerdrReporter, HerdrSession, HerdrState};

/// Reports a session's job state to Herdr when it changes.
pub(crate) struct HerdrStatus {
    reporter: HerdrReporter,
    session: HerdrSession,
    last: Option<HerdrState>,
}

impl HerdrStatus {
    pub(crate) fn new(reporter: &HerdrReporter, id: &SessionId) -> Self {
        Self {
            reporter: reporter.clone(),
            session: HerdrSession::without_resume(id.as_str()),
            last: None,
        }
    }

    pub(crate) async fn report(&mut self, state: JobState) {
        let state = herdr_state(state);
        if !self.reporter.is_enabled() || self.last == Some(state) {
            return;
        }
        self.last = Some(state);
        let _ = self
            .reporter
            .report_state(state, None, Some(&self.session))
            .await;
    }
}

/// The pane state Herdr shows for a session's job.
fn herdr_state(state: JobState) -> HerdrState {
    match state {
        JobState::Running | JobState::Queued => HerdrState::Working,
        JobState::WaitingForInput => HerdrState::Blocked,
        JobState::Idle
        | JobState::Completed
        | JobState::Failed
        | JobState::Cancelled
        | JobState::TimedOut
        | JobState::Interrupted => HerdrState::Idle,
        // `Unknown`, or a state a newer daemon sends.
        _ => HerdrState::Idle,
    }
}
