use pretty_assertions::assert_eq;
use serde_json::Value;

// Covers: the committed schema/protocol.json is what the wire types
// generate, so clients that generate types from it speak the daemon's wire.
// Owner: protocol schema.
#[test]
fn the_committed_schema_matches_the_wire_types() {
    let committed: Value = serde_json::from_str(include_str!("../schema/protocol.json"))
        .expect("schema/protocol.json is JSON");
    assert_eq!(
        committed,
        super::document(),
        "schema/protocol.json is stale; regenerate it with `cargo run -p rho-protocol --example protocol_schema > crates/rho-protocol/schema/protocol.json`"
    );
}
