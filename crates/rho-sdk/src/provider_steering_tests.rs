use pretty_assertions::assert_eq;

use super::{ProviderSteeringOutcome, ProviderSteeringRequest};

// Covers: explicit settlement followed by Drop must not emit a second outcome.
// Owner: SDK provider steering request lifecycle.
#[test]
fn settlement_emits_exactly_one_outcome() {
    let cases = [
        (
            ProviderSteeringRequest::accept as fn(ProviderSteeringRequest),
            ProviderSteeringOutcome::Accepted,
        ),
        (
            ProviderSteeringRequest::release,
            ProviderSteeringOutcome::Released,
        ),
        (drop, ProviderSteeringOutcome::Released),
    ];
    for (settle, expected) in cases {
        let (request, mut outcomes) = ProviderSteeringRequest::test_unclaimed(Vec::new());
        let id = request.id().clone();
        settle(request);
        assert_eq!(outcomes.try_recv(), Ok((id, expected)));
        assert_eq!(
            outcomes.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        );
    }
}
