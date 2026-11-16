use pretty_assertions::assert_eq;
use rho_sdk::ToolCallId;
use serde_json::{json, Value};

use super::*;

fn call(id: &str) -> ToolCallId {
    ToolCallId::from_string(id).unwrap()
}

fn text(text: &str) -> Value {
    json!({"version": "1.0", "updateComponents": {
        "surfaceId": "s",
        "components": [{"id": "root", "component": "Text", "text": text}],
    }})
}

// Covers: a surface is drawn once, under the latest call that showed it,
// and not at all once deleted; a call's repeated updates apply once.
// Owner: pure model
#[test]
fn surface_is_drawn_under_the_latest_call_that_showed_it() {
    let mut store = SurfaceStore::default();
    let create = json!({"version": "1.0", "createSurface": {
        "surfaceId": "s",
        "catalogId": Catalog::basic().id(),
    }});
    let delete = json!({"version": "1.0", "deleteSurface": {"surfaceId": "s"}});
    let steps = [
        ("first", vec![create, text("one")], true, Some("first")),
        ("first", vec![text("repeat")], false, Some("first")),
        ("second", vec![text("two")], true, Some("second")),
        ("third", vec![delete], true, None),
    ];
    for (id, messages, applies, drawn_under) in steps {
        assert_eq!(store.apply(&call(id), "s", &messages), applies, "{id}");
        assert_eq!(
            store.drawn_under("s").map(ToolCallId::as_str),
            drawn_under,
            "{id}"
        );
    }
}
