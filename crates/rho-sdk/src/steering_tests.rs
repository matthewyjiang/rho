use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

use super::{Delivery, SteeringQueue};
use crate::{
    model::{ContentBlock, Message},
    provider::ProviderSteeringOutcome,
    SteeringRetraction, UserInput,
};

fn outcomes() -> (
    mpsc::UnboundedSender<(crate::SteeringId, ProviderSteeringOutcome)>,
    mpsc::UnboundedReceiver<(crate::SteeringId, ProviderSteeringOutcome)>,
) {
    mpsc::unbounded_channel()
}

// Covers: delivered entries apply alone when mixed with later undelivered steers
// Owner: sdk steering queue
#[test]
fn apply_prefers_delivered_entries_when_any_exist() {
    let mut queue = SteeringQueue::new();
    let delivered = queue.accept(UserInput::text("s1"));
    let late = queue.accept(UserInput::text("s2"));
    queue.mark_delivered(&delivered);

    let mut history = Vec::new();
    let applied = queue.apply(&mut history);

    assert_eq!(applied, vec![delivered.clone()]);
    assert_eq!(history, vec![Message::user_text("s1")]);
    assert_eq!(queue.staged_ids(), vec![late]);
}

// Covers: with no delivered entries, every staged steer is applied
// Owner: sdk steering queue
#[test]
fn apply_all_staged_when_none_were_delivered() {
    let mut queue = SteeringQueue::new();
    let first = queue.accept(UserInput::text("s1"));
    let second = queue.accept(UserInput::text("s2"));

    let mut history = Vec::new();
    let applied = queue.apply(&mut history);

    assert_eq!(applied, vec![first, second]);
    assert_eq!(
        history,
        vec![Message::user_text("s1"), Message::user_text("s2")]
    );
    assert!(queue.staged_ids().is_empty());
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

// Covers: claim and retract race on one mutex; only one side wins
// Owner: sdk steering queue
#[test]
fn claim_and_retract_are_atomic() {
    let mut queue = SteeringQueue::new();
    let id = queue.accept(UserInput::text("s"));
    let (outcomes, _rx) = outcomes();
    let mut requests = queue.offer_unoffered(outcomes);
    let request = requests.pop().unwrap();

    assert!(request.claim());
    assert!(!request.claim());
    assert_eq!(queue.retract(&id), SteeringRetraction::AlreadyApplied);
    request.release();
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
