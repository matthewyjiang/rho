//! The typed side of the notification cards a turn boundary delivers
//! ([`NotificationInfo`]): what each card is about, one item per event it
//! tells of, and how urgently it wants the reader. Each source names its
//! subjects and outcomes; the urgency an outcome carries is decided here
//! once, so every source reads the same way.

use rho_protocol::display::{
    NotificationCard, NotificationId, NotificationInfo, NotificationItem, NotificationOutcome,
    NotificationSubject, NotificationUrgency,
};

use crate::display_transcript::NotificationRow;

impl NotificationRow {
    /// `card`, about `subject`, telling of `items`: under a new id, which
    /// the row keeps wherever it is saved and replayed, as urgent as its
    /// most urgent item.
    pub(crate) fn typed(
        card: NotificationCard,
        subject: NotificationSubject,
        items: Vec<NotificationItem>,
    ) -> Self {
        let urgency = items
            .iter()
            .map(|item| urgency(item.outcome))
            .max_by_key(|urgency| rank(*urgency))
            .unwrap_or(NotificationUrgency::Routine);
        let id = NotificationId::new(uuid::Uuid::new_v4().to_string());
        Self {
            card,
            info: Some(NotificationInfo::new(id, urgency, subject, items)),
        }
    }
}

/// What an event that ended as `outcome` asks of the reader: to act on a
/// question or a failure, to look at finished work, or nothing.
fn urgency(outcome: NotificationOutcome) -> NotificationUrgency {
    use NotificationOutcome as Outcome;
    match outcome {
        Outcome::Question
        | Outcome::ActionRequested
        | Outcome::Failed
        | Outcome::FailedToStart
        | Outcome::TimedOut
        | Outcome::ChecksFailed
        | Outcome::MergeConflict
        | Outcome::Unreadable => NotificationUrgency::NeedsYou,
        Outcome::Completed
        | Outcome::Exited
        | Outcome::Interrupted
        | Outcome::CutOff
        | Outcome::Refused
        | Outcome::OutOfSteps
        | Outcome::ChecksPassed
        | Outcome::ChecksFinished
        | Outcome::NewComments
        | Outcome::Merged
        | Outcome::Closed => NotificationUrgency::Notable,
        Outcome::Update
        | Outcome::Running
        | Outcome::Stopped
        | Outcome::Cancelled
        | Outcome::NewCommits
        | Outcome::WatchStopped => NotificationUrgency::Routine,
        // `Unknown`, and any outcome a newer protocol adds: no source here
        // sends them.
        _ => NotificationUrgency::Routine,
    }
}

/// Orders urgencies, the most urgent highest.
fn rank(urgency: NotificationUrgency) -> u8 {
    match urgency {
        NotificationUrgency::NeedsYou => 2,
        NotificationUrgency::Notable => 1,
        // `Routine`, `Unknown`, and any value a newer protocol adds.
        _ => 0,
    }
}
