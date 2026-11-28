use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use agent_client_protocol::schema::ProtocolVersion;

use super::InitializeScreen;

fn screen() -> InitializeScreen {
    InitializeScreen::new(&[ProtocolVersion::V1, ProtocolVersion::V2])
}

fn answer(screen: &InitializeScreen, line: Value) -> Option<Value> {
    screen
        .incoming(&line.to_string())
        .map(|reply| serde_json::from_str(&reply).unwrap())
}

fn initialize(id: Value, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": params})
}

fn invalid_params(id: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {
        "code": -32602,
        "message": "Invalid params",
        "data": "initialize.protocolVersion must be an integer from 0 to 65535",
    }})
}

#[test]
fn answers_only_the_initialize_requests_the_router_misroutes() {
    let info = json!({"name": "host", "version": "1"});
    let cases = [
        (
            "protocolVersion 0",
            initialize(json!(1), json!({"protocolVersion": 0, "info": info})),
            Some(json!({"jsonrpc": "2.0", "id": 1, "error": {
                "code": -32022,
                "message": "Unsupported protocol version",
                "data": {"requested": 0, "supported": [1, 2]},
            }})),
        ),
        (
            "protocolVersion omitted",
            initialize(json!("a"), json!({"info": info})),
            Some(invalid_params(json!("a"))),
        ),
        (
            "protocolVersion a string",
            initialize(json!(2), json!({"protocolVersion": "2"})),
            Some(invalid_params(json!(2))),
        ),
        (
            "protocolVersion out of range",
            initialize(json!(3), json!({"protocolVersion": 65536})),
            Some(invalid_params(json!(3))),
        ),
        (
            "no params",
            json!({"jsonrpc": "2.0", "id": 4, "method": "initialize"}),
            Some(invalid_params(json!(4))),
        ),
        (
            "v1",
            initialize(json!(5), json!({"protocolVersion": 1})),
            None,
        ),
        (
            "v2",
            initialize(json!(6), json!({"protocolVersion": 2, "info": info})),
            None,
        ),
        (
            "a newer version",
            initialize(json!(7), json!({"protocolVersion": 65535})),
            None,
        ),
        (
            "a null id, out of scope: the router takes it as it comes",
            initialize(Value::Null, json!({"protocolVersion": 0})),
            None,
        ),
        (
            "another method",
            json!({"jsonrpc": "2.0", "id": 8, "method": "session/new", "params": {}}),
            None,
        ),
        (
            "a notification",
            json!({"jsonrpc": "2.0", "method": "initialize", "params": {}}),
            None,
        ),
    ];
    for (case, line, expected) in cases {
        assert_eq!(answer(&screen(), line), expected, "{case}");
    }
    assert_eq!(
        screen().incoming("not json"),
        None,
        "a malformed line is the router's to answer"
    );
}

#[test]
fn hands_initialize_to_the_router_once_one_succeeds() {
    let screen = screen();
    let v2 = initialize(json!(1), json!({"protocolVersion": 2}));
    let zero = initialize(json!(2), json!({"protocolVersion": 0}));
    assert_eq!(answer(&screen, v2), None);

    // A failed initialize leaves the connection uninitialized.
    screen.outgoing(
        &json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "Invalid params"}})
            .to_string(),
    );
    assert!(answer(&screen, zero.clone()).is_some());

    // A result for a request the router never got settles nothing.
    screen.outgoing(&json!({"jsonrpc": "2.0", "id": 9, "result": {}}).to_string());
    assert!(answer(&screen, zero.clone()).is_some());

    screen.outgoing(
        &json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": 2}}).to_string(),
    );
    assert_eq!(
        answer(&screen, zero),
        None,
        "a repeat goes to the selected version"
    );
}
