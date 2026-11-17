use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn remote(name: &str) -> Remote {
    Remote {
        name: name.to_owned(),
        url: format!("http://10.0.0.2:8765/{name}"),
        secret: "rho_mcp_secret".to_owned(),
    }
}

/// The machines of a hub called `hub` with `remotes`.
fn hub(remotes: Result<Vec<Remote>, &str>) -> Machines {
    Machines {
        local: "hub".to_owned(),
        remotes: remotes
            .map(|remotes| RemoteSet { remotes })
            .map_err(str::to_owned),
    }
}

fn arguments(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(arguments) => arguments,
        _ => panic!("arguments are an object"),
    }
}

// Covers: with no remotes every call is the server's own, naming it or
// not; with remotes each call names a server, this one or a remote, a
// `list` that names none lists every one, an `unwatch` that names none is
// this server's, and a missing, unknown, or ambiguous name is refused with
// the names there are, or why the remotes could not be read. `server`
// never reaches the core.
// Owner: hub routing.
#[test]
fn calls_route_by_server_and_name_one_once_there_are_remotes() {
    let none = hub(Ok(Vec::new()));
    let some = hub(Ok(vec![remote("lab-box"), remote("laptop")]));
    let shadowing = hub(Ok(vec![remote("hub")]));
    let unreadable = hub(Err("keyring locked"));
    let route = |machines: &Machines, tool: ServedTool, value: Value| {
        let mut arguments = arguments(value);
        let routed = machines
            .route(tool, &mut arguments)
            .map_err(|error| error.to_string());
        (routed, arguments.contains_key(SERVER))
    };

    assert_eq!(
        [
            route(&none, ServedTool::Core(Action::New), json!({"prompt": "go"})),
            route(&none, ServedTool::Core(Action::New), json!({"server": "hub"})),
            route(&none, ServedTool::Core(Action::New), json!({"server": "lab-box"})),
            route(&some, ServedTool::Core(Action::List), json!({})),
            route(&some, ServedTool::Core(Action::List), json!({"server": "laptop"})),
            route(&some, ServedTool::Unwatch, json!({"session": "s1"})),
            route(&some, ServedTool::Core(Action::New), json!({"server": "hub"})),
            route(&some, ServedTool::Core(Action::New), json!({})),
            route(&some, ServedTool::Core(Action::Wait), json!({"server": 7})),
            route(&shadowing, ServedTool::Core(Action::Read), json!({"server": "hub"})),
            route(&unreadable, ServedTool::Core(Action::Read), json!({"server": "lab-box"})),
            route(&unreadable, ServedTool::Core(Action::Read), json!({})),
        ],
        [
            (Ok(Route::One(Target::Local)), false),
            (Ok(Route::One(Target::Local)), false),
            (Err("no server \"lab-box\"; servers: hub".to_owned()), false),
            (Ok(Route::Every), false),
            (Ok(Route::One(Target::Remote(remote("laptop")))), false),
            (Ok(Route::One(Target::Local)), false),
            (Ok(Route::One(Target::Local)), false),
            (
                Err(
                    "`new` needs `server`, the machine to act on: one of hub, lab-box, laptop"
                        .to_owned()
                ),
                false
            ),
            (
                Err("invalid `wait` arguments: `server` is a server name, not 7".to_owned()),
                false
            ),
            (
                Err("remote \"hub\" has this server's own name, so `server` cannot tell them apart; rename the remote with `rho mcp remote add`".to_owned()),
                false
            ),
            (
                Err("could not read this server's remotes, so it cannot reach server \"lab-box\": keyring locked".to_owned()),
                false
            ),
            (Ok(Route::One(Target::Local)), false),
        ]
    );
}

// Covers: a hub's tools offer `server` only once it has remotes, naming
// every machine once, required for each tool but `list` and `unwatch`.
// Owner: hub tool schemas.
#[test]
fn tools_take_a_server_once_there_are_remotes() {
    let some = hub(Ok(vec![remote("lab-box"), remote("hub")]));
    let schema = || {
        arguments(json!({
            "type": "object",
            "properties": {"session": {"type": "string"}},
            "required": ["session"],
        }))
    };
    let server_of = |schema: &Map<String, Value>| {
        (
            schema["properties"]
                .get(SERVER)
                .map(|server| server["enum"].clone()),
            schema.get("required").cloned(),
        )
    };

    assert_eq!(
        [
            server_of(&hub(Ok(Vec::new())).with_server(ServedTool::Core(Action::Read), schema())),
            server_of(&some.with_server(ServedTool::Core(Action::Read), schema())),
            server_of(&some.with_server(ServedTool::Core(Action::List), schema())),
            server_of(&some.with_server(ServedTool::Unwatch, schema())),
        ],
        [
            (None, Some(json!(["session"]))),
            (
                Some(json!(["hub", "lab-box"])),
                Some(json!(["session", "server"]))
            ),
            (Some(json!(["hub", "lab-box"])), Some(json!(["session"]))),
            (Some(json!(["hub", "lab-box"])), Some(json!(["session"]))),
        ]
    );
}
