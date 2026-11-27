use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: a sealed bundle and a comparison travel in their wire form: the
// suite's name, the item tag, and the compare states. Reading unknown
// values is `schema_tests`'s.
// Owner: protocol wire contract.
#[test]
fn sync_messages_travel_in_their_wire_form() {
    let item = SecretItem::ApiKey {
        auth: "openai".into(),
    };
    let sealed = SealedSecrets::new(
        SealSuite::X25519HkdfSha256ChaCha20Poly1305,
        "cGs=".into(),
        vec![item.clone()],
        "ZW5j".into(),
        "Y3Q=".into(),
    );
    let sealed_wire = json!({
        "suite": "x25519-hkdf-sha256-chacha20poly1305",
        "recipient": "cGs=",
        "items": [{"type": "api_key", "auth": "openai"}],
        "encapped_key": "ZW5j",
        "ciphertext": "Y3Q=",
    });
    assert_eq!(serde_json::to_value(&sealed).unwrap(), sealed_wire);
    assert_eq!(
        serde_json::from_value::<SealedSecrets>(sealed_wire).unwrap(),
        sealed
    );

    let compared = SyncCompareResponse::new(vec![
        SecretComparison::new(item.clone(), CompareState::Same),
        SecretComparison::new(item.clone(), CompareState::Differs),
        SecretComparison::new(item.clone(), CompareState::Absent),
    ]);
    let compared_wire = json!({"items": [
        {"item": {"type": "api_key", "auth": "openai"}, "state": "same"},
        {"item": {"type": "api_key", "auth": "openai"}, "state": "differs"},
        {"item": {"type": "api_key", "auth": "openai"}, "state": "absent"},
    ]});
    assert_eq!(serde_json::to_value(&compared).unwrap(), compared_wire);
    assert_eq!(
        serde_json::from_value::<SyncCompareResponse>(compared_wire).unwrap(),
        compared
    );
}
