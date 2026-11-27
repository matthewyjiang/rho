//! The typed side of the notification cards a turn boundary delivers
//! ([`NotificationInfo`]): what each card is about, one item per event it
//! tells of, and how urgently it wants the reader. Each source names its
//! subjects, outcomes and tones; the urgency an outcome carries is decided
//! here once, so every source reads the same way.

use rho_protocol::display::{
    NotificationCard, NotificationId, NotificationInfo, NotificationItem, NotificationOutcome,
    NotificationSubject, NotificationTone, NotificationUrgency,
};

use crate::display_transcript::NotificationRow;

impl NotificationRow {
    /// `card`, about `subject`, telling of `items`, each built with
    /// [`notification_item`]: under a new id, which the row keeps wherever it
    /// is saved and replayed, as urgent as its most urgent item.
    pub(crate) fn typed(
        card: NotificationCard,
        subject: NotificationSubject,
        items: Vec<NotificationItem>,
    ) -> Self {
        let most_urgent = items
            .iter()
            .map(|item| item.urgency)
            .max_by_key(|urgency| rank(*urgency))
            .unwrap_or(NotificationUrgency::Routine);
        let id = NotificationId::new(uuid::Uuid::new_v4().to_string());
        Self {
            card,
            info: Some(NotificationInfo::new(id, most_urgent, subject, items)),
        }
    }

    /// `card`, about `subject`, telling of one event that ended as
    /// `outcome`, which its title says, in the card's tone.
    pub(crate) fn single(
        card: NotificationCard,
        subject: NotificationSubject,
        outcome: NotificationOutcome,
    ) -> Self {
        let item = notification_item(outcome, card.tone, subject.clone(), card.title.clone());
        Self::typed(card, subject, vec![item])
    }
}

/// One event a card tells of, that ended as `outcome`, as urgent as that
/// outcome is, in the `tone` its source gives it: how every source builds
/// its items.
pub(crate) fn notification_item(
    outcome: NotificationOutcome,
    tone: NotificationTone,
    subject: NotificationSubject,
    text: impl Into<String>,
) -> NotificationItem {
    NotificationItem::new(outcome, urgency(outcome), tone, subject, text)
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
