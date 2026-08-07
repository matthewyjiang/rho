use pretty_assertions::assert_eq;

use crate::agent::OneShotPhase;

use super::{decode, encode};

// Covers: empty and non-empty bodies round-trip without leaking status into body
// Owner: advisor progress codec
#[test]
fn progress_snapshots_round_trip() {
    assert_eq!(
        decode(&encode(OneShotPhase::WaitingForProvider, "")),
        ("waiting for provider", "")
    );
    assert_eq!(
        decode(&encode(OneShotPhase::Responding, "do this\nthen that")),
        ("responding", "do this\nthen that")
    );
}
