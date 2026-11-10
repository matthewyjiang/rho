use pretty_assertions::assert_eq;
use serde_json::{json, Map, Value};

use super::Call;
use crate::app::session_control::Action;

fn arguments(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(arguments) => arguments,
        _ => unreachable!("the table's arguments are objects"),
    }
}

// Covers: a `read` routes to the daemon's live transcript unless it names
// an `anchor`, which reads saved history; an argument a model fills with
// `null`, `anchor` included, counts as absent, so it neither misroutes the
// call nor reaches the core as an argument it does not take (#1714).
// Owner: native sessions tool's routing between history and the core.
#[test]
fn null_arguments_count_as_absent_when_routing() {
    let cases = [
        (
            json!({"action": "read", "session": "s", "anchor": null, "query": null}),
            Some(Call {
                action: Action::Read,
                arguments: arguments(json!({"session": "s"})),
            }),
        ),
        (
            json!({"action": "read", "session": "s", "anchor": "a"}),
            None,
        ),
        (
            json!({"action": "search", "query": "q", "workspace": null}),
            None,
        ),
        (
            json!({"action": "wait", "session": "s", "cursor": null}),
            Some(Call {
                action: Action::Wait,
                arguments: arguments(json!({"session": "s"})),
            }),
        ),
    ];

    for (given, routed) in cases {
        assert_eq!(Call::parse(&given), routed, "{given}");
    }
}
