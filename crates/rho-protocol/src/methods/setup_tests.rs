use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: a daemon built before surfaces refuses a setup part it does not
// know and ignores a setup field it does not know, so asking for surfaces
// must be a field of its own, and a client that draws none must send
// nothing of them; a client built before surfaces sends no field and must
// get a session without them
// Owner: protocol wire format
#[test]
fn asking_for_surfaces_is_one_setup_field_an_older_daemon_ignores() {
    let drawn = SessionSetup {
        surfaces: Surfaces::Drawn,
        ..SessionSetup::default()
    };

    assert_eq!(
        (
            serde_json::to_value(SessionSetup::default()).unwrap(),
            serde_json::to_value(&drawn).unwrap(),
            serde_json::from_value::<SessionSetup>(json!({})).unwrap(),
        ),
        (
            json!({}),
            json!({"surfaces": "drawn"}),
            SessionSetup::default(),
        ),
    );
}
