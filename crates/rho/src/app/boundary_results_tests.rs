use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_protocol::display::{
    NotificationOutcome, NotificationSubject, NotificationTone, NotificationUrgency,
};

use super::BoundaryResults;
use crate::{
    app::{peer_sessions::PeerBatch, pull_requests::NewsBatch, subagent_manager::SubagentSnapshot},
    display_transcript::DisplayRow,
    subagent::{RunState, RunStatus},
    tools::agent::SubagentNotification,
};

/// A subagent run `id` that ended in `state`.
fn ended(id: &str, state: RunState) -> SubagentNotification {
    SubagentNotification {
        snapshot: SubagentSnapshot {
            prior_notices: Vec::new(),
            persistence: crate::session::SessionPersistence::Saved,
            id: id.into(),
            agent_id: "worker".into(),
            title: Some("Fix the build".into()),
            elapsed: Duration::from_secs(5),
            status: RunStatus {
                state,
                turns: 1,
                result: Some("done".into()),
                ..RunStatus::default()
            },
            done: true,
        },
    }
}

// Covers: a subagent's result card is about its run, says how the run
// ended, and wants the reader as that ending does: a failure needs them, a
// finished run is notable, a stopped one routine. A miss files a failed
// worker among routine cards.
// Owner: boundary result rendering.
#[test]
fn a_subagent_results_card_is_as_urgent_as_its_ending() {
    use NotificationOutcome as Outcome;
    use NotificationUrgency as Urgency;
    let cases = [
        (
            "ok",
            RunState::Ok,
            NotificationTone::Success,
            Outcome::Completed,
            Urgency::Notable,
        ),
        (
            "error",
            RunState::Error,
            NotificationTone::Error,
            Outcome::Failed,
            Urgency::NeedsYou,
        ),
        (
            "stopped",
            RunState::Stopped,
            NotificationTone::Accent,
            Outcome::Stopped,
            Urgency::Routine,
        ),
    ];
    let subagents: Vec<_> = cases
        .iter()
        .map(|(id, state, ..)| ended(id, *state))
        .collect();
    let (_, transcript) = BoundaryResults {
        notices: &[],
        subagents: &subagents,
        workflows: &[],
        processes: &[],
        surface_actions: &[],
        pull_requests: &NewsBatch::default(),
        peer_sessions: &PeerBatch::default(),
    }
    .render(/*manager*/ None)
    .finish()
    .expect("the results render");

    let read: Vec<_> = transcript
        .0
        .into_iter()
        .map(|row| {
            let DisplayRow::Notification(row) = row else {
                panic!("a result renders as a card, not {row:?}");
            };
            let info = row.info.expect("a result is typed");
            let outcomes: Vec<_> = info.items.iter().map(|item| item.outcome).collect();
            (row.card.tone, info.urgency, info.subject, outcomes)
        })
        .collect();
    let expected: Vec<_> = cases
        .into_iter()
        .map(|(id, _, tone, outcome, urgency)| {
            let subject = NotificationSubject::Subagent {
                run: id.into(),
                session: None,
            };
            (tone, urgency, subject, vec![outcome])
        })
        .collect();
    assert_eq!(read, expected);
}
