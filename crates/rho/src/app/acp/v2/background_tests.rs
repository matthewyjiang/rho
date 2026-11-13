use pretty_assertions::assert_eq;
use rho_protocol::{ProcessState, SessionProcess};
use serde_json::{json, Value};

use super::changes;

fn process(id: &str, state: ProcessState, exit_code: Option<i32>) -> SessionProcess {
    SessionProcess {
        id: id.to_owned(),
        command: format!("run {id}"),
        state,
        elapsed_seconds: 0,
        quiet_seconds: None,
        exit_code,
    }
}

// Covers: a host must hear of a terminal before a state lists it, and of
// its exit before a state drops it; a process it never saw running is never
// announced, and one that left without a seen end exited unknown.
// Owner: ACP v2 background mapping
#[test]
fn terminals_are_announced_on_start_and_finalized_on_stop() {
    use ProcessState::{Exited, Running, Starting, TimedOut};
    let announce = |id: &str| json!({"sessionUpdate": "terminal_update", "terminalId": id, "command": format!("run {id}")});
    let exit = |id: &str, status: Value| json!({"sessionUpdate": "terminal_update", "terminalId": id, "exitStatus": status});
    let cases = [
        (
            "starting and running both announce once",
            vec![process("a", Starting, None)],
            vec![process("a", Running, None), process("b", Running, None)],
            vec![announce("b")],
        ),
        (
            "an exit finalizes with its code",
            vec![process("a", Running, None)],
            vec![process("a", Exited, Some(3))],
            vec![exit("a", json!({"exitCode": 3}))],
        ),
        (
            "a stop with no code finalizes with no known status",
            vec![process("a", Running, None)],
            vec![process("a", TimedOut, None)],
            vec![exit("a", json!({}))],
        ),
        (
            "a running process that left unseen finalizes with no known status",
            vec![process("a", Running, None)],
            vec![],
            vec![exit("a", json!({}))],
        ),
        (
            "one that started and ended between reports is never listed",
            vec![],
            vec![process("a", Exited, Some(0))],
            vec![],
        ),
        (
            "an ended process leaving the list was already finalized",
            vec![process("a", Exited, Some(0))],
            vec![],
            vec![],
        ),
    ];
    for (case, previous, current, expected) in cases {
        let updates: Vec<Value> = changes(&previous, &current)
            .iter()
            .map(|update| serde_json::to_value(update).unwrap())
            .collect();
        assert_eq!(updates, expected, "{case}");
    }
}
