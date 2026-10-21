use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::{Delivery, SteeringQueue};
use crate::{
    model::{ContentBlock, Message},
    provider::ProviderSteeringOutcome,
    SteeringRetraction, UserInput,
};

type OutcomePort = (
    mpsc::UnboundedSender<(crate::SteeringId, ProviderSteeringOutcome)>,
    mpsc::UnboundedReceiver<(crate::SteeringId, ProviderSteeringOutcome)>,
);

fn outcomes() -> OutcomePort {
    mpsc::unbounded_channel()
}

// Covers: delivered entries apply alone when mixed with later undelivered
// steers; with none delivered, every staged steer applies
// Owner: sdk steering queue
#[test]
fn apply_prefers_delivered_entries_when_any_exist() {
    for deliver_first in [true, false] {
        let mut queue = SteeringQueue::new();
        let first = queue.accept(UserInput::text("s1"));
        let second = queue.accept(UserInput::text("s2"));
        if deliver_first {
            queue.mark_delivered(&first);
        }

        let mut history = Vec::new();
        let applied = queue.apply(&mut history);

        let (expected_applied, expected_history, expected_staged) = if deliver_first {
            (vec![first], vec![Message::user_text("s1")], vec![second])
        } else {
            (
                vec![first, second],
                vec![Message::user_text("s1"), Message::user_text("s2")],
                vec![],
            )
        };
        assert_eq!(
            (applied, history, queue.staged_ids()),
            (expected_applied, expected_history, expected_staged),
            "deliver_first={deliver_first}"
        );
    }
}

// Covers: retraction is atomic against claim; delivered input is too late
// Owner: sdk steering queue
#[test]
fn retraction_table_offered_claimed_and_delivered() {
    let cases = [
        "offered-unclaimed",
        "offered-claimed",
        "delivered",
        "staged",
        "unknown",
    ];
    for case in cases {
        let mut queue = SteeringQueue::new();
        let (outcomes, _rx) = outcomes();
        let result = match case {
            "staged" => {
                let id = queue.accept(UserInput::text("s"));
                (queue.retract(&id), SteeringRetraction::Retracted)
            }
            "offered-unclaimed" => {
                let id = queue.accept(UserInput::text("s"));
                let requests = queue.offer_unoffered(outcomes);
                assert_eq!(requests.len(), 1);
                (queue.retract(&id), SteeringRetraction::Retracted)
            }
            "offered-claimed" => {
                let id = queue.accept(UserInput::text("s"));
                let mut requests = queue.offer_unoffered(outcomes);
                assert!(requests[0].claim());
                assert!(!requests[0].claim(), "a request is claimed at most once");
                let retraction = queue.retract(&id);
                requests.pop().unwrap().release();
                (retraction, SteeringRetraction::AlreadyApplied)
            }
            "delivered" => {
                let id = queue.accept(UserInput::text("s"));
                queue.mark_delivered(&id);
                (queue.retract(&id), SteeringRetraction::AlreadyApplied)
            }
            "unknown" => (
                queue.retract(&crate::SteeringId::new()),
                SteeringRetraction::NotFound,
            ),
            _ => unreachable!(),
        };
        assert_eq!(result.0, result.1, "{case}");
    }
}

// Covers: a failed provider turn restages delivered input so it can be re-offered
// Owner: sdk steering queue
#[test]
fn reset_delivery_restages_delivered_and_offered() {
    let mut queue = SteeringQueue::new();
    let first = queue.accept(UserInput::text("s1"));
    let second = queue.accept(UserInput::text("s2"));
    let (first_outcomes, _rx) = outcomes();
    let requests = queue.offer_unoffered(first_outcomes);
    queue.mark_delivered(&first);
    drop(requests);

    queue.reset_delivery();

    assert!(queue
        .staged
        .iter()
        .all(|entry| matches!(entry.delivery, Delivery::Staged)));
    let (reoffer, _rx) = outcomes();
    let requests = queue.offer_unoffered(reoffer);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].id(), &first);
    assert_eq!(requests[1].id(), &second);
    assert_eq!(
        requests[0].content(),
        [ContentBlock::Text("s1".into())].as_slice()
    );
}
