use pretty_assertions::assert_eq;
use rho_protocol::{BackgroundItem, BackgroundStatus, BackgroundWork, SessionId};
use serde_json::{json, Value};

use super::{changes, subagent, terminal, tool_call};

fn process(id: &str, status: BackgroundStatus, exit_code: Option<i32>) -> BackgroundWork {
    BackgroundWork {
        item: BackgroundItem::Process { id: id.to_owned() },
        title: format!("run {id}"),
        status,
        exit_code,
    }
}

fn workflow(run: &str, status: BackgroundStatus) -> BackgroundWork {
    BackgroundWork {
        item: BackgroundItem::Workflow {
            run: run.to_owned(),
        },
        title: "build".to_owned(),
        status,
        exit_code: None,
    }
}

fn subagent_work(run: &str, session: Option<&str>, status: BackgroundStatus) -> BackgroundWork {
    BackgroundWork {
        item: BackgroundItem::Subagent {
            run: run.to_owned(),
            session: session.map(SessionId::new),
        },
        title: "review".to_owned(),
        status,
        exit_code: None,
    }
}

// Covers: a host must hear of each piece of work as its v2 entity, with the
// capability to cancel it, before a state lists it, and of how it ended,
// with that capability withdrawn, before a state drops it; work it never
// saw running is never announced, and work that left unseen ends as far as
// the host can tell.
// Owner: ACP v2 background mapping
#[test]
fn work_is_announced_on_start_and_finalized_on_stop() {
    use BackgroundStatus::{Cancelled, Completed, Failed, Running};
    let cancel = json!({"cancel": {}});
    let cases = [
        (
            "a process that starts is a cancellable terminal",
            vec![process("a", Running, None)],
            vec![process("a", Running, None), process("b", Running, None)],
            vec![json!({
                "sessionUpdate": "terminal_update", "terminalId": "b", "command": "run b",
                "capabilities": cancel,
            })],
        ),
        (
            "an exit finalizes the terminal with its code",
            vec![process("a", Running, None)],
            vec![process("a", Completed, Some(3))],
            vec![json!({
                "sessionUpdate": "terminal_update", "terminalId": "a",
                "exitStatus": {"exitCode": 3}, "capabilities": null,
            })],
        ),
        (
            "a process that left unseen exited with no known status",
            vec![process("a", Running, None)],
            vec![],
            vec![json!({
                "sessionUpdate": "terminal_update", "terminalId": "a", "exitStatus": {},
                "capabilities": null,
            })],
        ),
        (
            "a workflow run is a tool call rho starts, then gives its final status",
            vec![workflow("w", Running)],
            vec![workflow("w", Cancelled), workflow("x", Running)],
            vec![
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": "rho-workflow-x",
                    "title": "build", "kind": "other", "status": "in_progress",
                    "capabilities": cancel,
                }),
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": "rho-workflow-w",
                    "status": "cancelled", "capabilities": null,
                }),
            ],
        ),
        (
            "a subagent is a child session, by its own session or its run",
            vec![subagent_work("s", Some("child"), Running)],
            vec![
                subagent_work("s", Some("child"), Failed),
                subagent_work("t", None, Running),
            ],
            vec![
                json!({
                    "sessionUpdate": "subagent_update", "sessionId": "t", "title": "review",
                    "state": {"state": "running"}, "capabilities": cancel,
                }),
                json!({
                    "sessionUpdate": "subagent_update", "sessionId": "child",
                    "state": {"state": "idle", "stopReason": "error"}, "capabilities": null,
                }),
            ],
        ),
        (
            "work that started and ended between reports is never listed",
            vec![],
            vec![process("a", Completed, Some(0)), workflow("w", Completed)],
            vec![],
        ),
        (
            "ended work leaving the list was already finalized",
            vec![process("a", Completed, Some(0))],
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

// Covers: a cancel names running work only by the entity the host was told
// of; finished work and other kinds' ids are not cancellable.
// Owner: ACP v2 background mapping
#[test]
fn cancels_find_only_running_work_by_its_entity() {
    use BackgroundStatus::{Completed, Running};
    let work = [
        process("p", Running, None),
        process("done", Completed, Some(0)),
        workflow("w", Running),
        subagent_work("s", Some("child"), Running),
    ];
    assert_eq!(
        (
            terminal(&work, "p"),
            terminal(&work, "done"),
            terminal(&work, "rho-workflow-w"),
            tool_call(&work, "rho-workflow-w"),
            tool_call(&work, "w"),
            subagent(&work, "child"),
            subagent(&work, "s"),
        ),
        (
            Some(BackgroundItem::Process { id: "p".into() }),
            None,
            None,
            Some(BackgroundItem::Workflow { run: "w".into() }),
            None,
            Some(BackgroundItem::Subagent {
                run: "s".into(),
                session: Some(SessionId::new("child")),
            }),
            None,
        )
    );
}
