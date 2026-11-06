//! Crash recovery of a workflow agent node through the real `rho` binary.
//!
//! The fixture provider (`RHO_AUTOMATION_TEST_MODE`) stands in for the model so
//! the process can be killed at a known step and resumed in another process.
#![cfg(unix)]

use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

const MODE_ENV: &str = "RHO_AUTOMATION_TEST_MODE";
const COMMAND_ENV: &str = "RHO_AUTOMATION_TEST_COMMAND";
/// The `process-then-delay` fixture holds its second model request for 30 s,
/// and a process that lives as long as the test's workspace holds a
/// `process-then-reply` prompt. The continue test, every row and run
/// included, measured 0.37-0.42 s over 5 local runs, and the reply test
/// 0.12-0.14 s. Giving up well before 30 s keeps a missing save a clear
/// failure instead of a run that quietly completes.
const SAVE_DEADLINE: Duration = Duration::from_secs(25);
/// The fixture's reply when no response is configured.
const FIXTURE_REPLY: &str = "fixture response";

const CONFIG: &str = r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"

[web_search]
hosted = false
provider = "disabled"
"#;

const WORKFLOW: &str = r#"def build(inputs):
    return workflow(name = "crash-recovery", nodes = [
        agent(
            name = "inspect",
            agent = "fixture",
            prompt = "inspect the workspace",
            access = "mutating",
            timeout_seconds = 600,
            max_output_bytes = 100000,
        ),
    ])

WORKFLOW = define(inputs = {}, build = build)
"#;

const AGENT: &str = "---
description: Fixture agent for crash recovery
tools: [process]
---

Do the task.
";

/// What the killed run left of the node's session for its resume.
#[derive(Clone, Copy, Debug)]
enum SavedSession {
    /// The session as saved, with the finished tool call.
    Kept,
    /// No session file, as when the run died before its first save.
    Removed,
    /// The session as saved, its header rebound to another agent.
    Rebound,
}

// Covers: a Rho agent node killed mid-run continues the same attempt by
// reopening the session it recorded, so the model sees the saved history
// (including the finished tool call) and not the prompt a second time, or
// only the prompt when nothing was saved; a session that no longer belongs
// to the node's frozen agent fails the attempt instead of running as
// another agent; `--dry-run` predicts the session it reopens.
// Owner: workflow crash recovery (the session id the attempt records,
// reopened through the headless agent runner).
#[test]
fn killed_agent_node_continues_its_saved_session() {
    for (saved, expected) in [
        (
            SavedSession::Kept,
            json!({
                "outcome": "success",
                "request": ["System", "User", "EnrichedAssistant", "ToolResult"],
            }),
        ),
        (
            SavedSession::Removed,
            json!({"outcome": "success", "request": ["System", "User"]}),
        ),
        (
            SavedSession::Rebound,
            json!({"outcome": "failure", "request": null}),
        ),
    ] {
        let root = workspace();
        let plan_id = plan(&root);

        // Turn one starts a process; turn two hangs, so the run is killed
        // after the save that holds the finished tool call. The process
        // outlives the kill: one that exited would add its exit notice to
        // the session in a race with the kill.
        let save = kill_after_save(
            &root,
            &plan_id,
            "process-then-delay",
            &held_process(&root),
            "\"ToolResult\"",
        );
        match saved {
            SavedSession::Kept => {}
            SavedSession::Removed => std::fs::remove_dir_all(&save.session).unwrap(),
            SavedSession::Rebound => rebind_header(&save.session, "other-agent"),
        }

        let preview = rho(
            &root,
            "inspect",
            &[
                "workflow",
                "resume",
                &save.run_id,
                "--dry-run",
                "--output",
                "jsonl",
            ],
        );
        let preview: Value = serde_json::from_slice(&success(&preview).stdout).unwrap();
        assert_eq!(
            preview,
            json!({
                "run_id": save.run_id,
                "needs_confirmation": true,
                "attempts": [{
                    "node": "s0.inspect",
                    "attempt": 1,
                    "action": "continue",
                    "session": save.session_id,
                }],
            }),
            "{saved:?}"
        );

        let outcome = resume(&root, &save.run_id, "inspect");
        // The inspect fixture answers with the request it received.
        let request = (outcome == "success").then(|| {
            let answer = std::fs::read(save.attempt.join("agent/answer.txt")).unwrap();
            let request: Value = serde_json::from_slice(&answer).unwrap();
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|message| message_kind(message).to_owned())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            json!({"outcome": outcome, "request": request}),
            expected,
            "{saved:?}"
        );
    }
}

// Covers: a node whose process died after its session saved the agent's
// final reply, but before the attempt finished, finishes with that reply
// on resume instead of asking the model again.
// Owner: workflow crash recovery (the headless agent runner's reopen).
#[test]
fn killed_agent_node_with_its_reply_saved_finishes_without_the_model() {
    let root = workspace();
    let plan_id = plan(&root);
    // Turn one starts a process and turn two replies; the live process holds
    // the prompt after the reply is saved, so the run is killed there.
    let save = kill_after_save(
        &root,
        &plan_id,
        "process-then-reply",
        &held_process(&root),
        FIXTURE_REPLY,
    );

    // The `fail` fixture fails every model request, so only a node that
    // asks the model nothing can succeed.
    assert_eq!(resume(&root, &save.run_id, "fail"), "success");
    assert_eq!(
        std::fs::read_to_string(save.attempt.join("agent/answer.txt")).unwrap(),
        FIXTURE_REPLY
    );
}

/// A command that runs until `root` is removed. A killed run leaves its
/// process behind, so it ends with the workspace instead of outliving the
/// test.
fn held_process(root: &TempDir) -> String {
    format!(
        "while [ -d '{}' ]; do sleep 0.1; done",
        root.path().display()
    )
}

/// A workspace holding the fixture workflow, its agent, and Rho's config.
fn workspace() -> TempDir {
    let root = TempDir::new().unwrap();
    std::fs::create_dir_all(root.path().join("wf/agents")).unwrap();
    std::fs::write(root.path().join("config.toml"), CONFIG).unwrap();
    std::fs::write(root.path().join("wf/main.star"), WORKFLOW).unwrap();
    std::fs::write(root.path().join("wf/agents/fixture.md"), AGENT).unwrap();
    root
}

/// Plans the fixture workflow and returns the plan's id.
fn plan(root: &TempDir) -> String {
    let plan = rho(
        root,
        "inspect",
        &["workflow", "plan", "wf/main.star", "--output", "json"],
    );
    let plan: Value = serde_json::from_slice(&success(&plan).stdout).unwrap();
    plan["manifest"]["plan_id"].as_str().unwrap().to_owned()
}

/// Runs `plan_id` with the `mode` fixture, its process tool starting
/// `process`, and kills it once attempt 1's session saved a record holding
/// `marker`.
fn kill_after_save(
    root: &TempDir,
    plan_id: &str,
    mode: &str,
    process: &str,
    marker: &str,
) -> AttemptSave {
    let mut run = command(root, mode);
    run.env(COMMAND_ENV, process)
        .args(["workflow", "run", plan_id, "--yes", "--output", "jsonl"]);
    let mut run = run.spawn().unwrap();
    let save = wait_for_save(root, marker);
    run.kill().unwrap();
    run.wait().unwrap();
    save
}

/// Resumes `run_id` with the `mode` fixture, confirming no prior process
/// remains, and returns the outcome its one node finished with.
fn resume(root: &TempDir, run_id: &str, mode: &str) -> Value {
    let resumed = rho(
        root,
        mode,
        &[
            "workflow",
            "resume",
            run_id,
            "--recover-uncertain",
            "--yes",
            "--output",
            "jsonl",
        ],
    );
    let node_events = String::from_utf8(success(&resumed).stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| {
            matches!(
                event["type"].as_str(),
                Some("node_started" | "node_finished")
            )
        })
        .map(|event| json!([event["type"], event["attempt"], event["outcome"]]))
        .collect::<Vec<_>>();
    let [started, finished] = node_events.as_slice() else {
        panic!("the node did not start and finish once: {node_events:?}");
    };
    assert_eq!(started, &json!(["node_started", 1, null]), "{mode}");
    assert_eq!(finished[0], "node_finished", "{mode}");
    finished[2].clone()
}

/// A provider request message's kind, which tags it.
fn message_kind(message: &Value) -> &str {
    let tags = message.as_object().unwrap();
    assert_eq!(tags.len(), 1, "{message}");
    tags.keys().next().unwrap()
}

/// Rewrites the header of the session in `session` to name `agent`, as if
/// the session had been rebound to it.
fn rebind_header(session: &Path, agent: &str) {
    let path = session.join("session.jsonl");
    let saved = std::fs::read_to_string(&path).unwrap();
    let (header, records) = saved.split_once('\n').unwrap();
    let mut header: Value = serde_json::from_str(header).unwrap();
    assert_eq!(header["type"], "session");
    header["agent_id"] = json!(agent);
    std::fs::write(&path, format!("{header}\n{records}")).unwrap();
}

/// Where attempt 1 of a run saved its agent's session.
struct AttemptSave {
    run_id: String,
    attempt: PathBuf,
    /// The session's id, which the attempt's `status.json` records.
    session_id: String,
    /// The session's directory below its workspace's.
    session: PathBuf,
}

/// Waits until attempt 1's agent has saved its session with a record that
/// holds `marker`.
fn wait_for_save(root: &TempDir, marker: &str) -> AttemptSave {
    let started = Instant::now();
    loop {
        if let Some(found) = saved_with(root.path(), marker) {
            return found;
        }
        assert!(
            started.elapsed() < SAVE_DEADLINE,
            "no record with {marker} saved within {SAVE_DEADLINE:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn saved_with(root: &Path, marker: &str) -> Option<AttemptSave> {
    let run = std::fs::read_dir(root.join(".rho/workflows/runs"))
        .ok()?
        .next()?
        .ok()?;
    let attempt = run.path().join("nodes/s0.inspect/attempts/1");
    let record: Value =
        serde_json::from_slice(&std::fs::read(attempt.join("status.json")).ok()?).ok()?;
    let session_id = record["state"]["agent_session"].as_str()?.to_owned();
    // Each session is a directory named for its id, below its workspace's.
    let suffix = format!("_{session_id}");
    let session = std::fs::read_dir(root.join(".rho/sessions"))
        .ok()?
        .flatten()
        .filter_map(|workspace| std::fs::read_dir(workspace.path()).ok())
        .flatten()
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))?
        .path();
    // Saves append one record per write, and readers ignore a truncated final
    // record, so only newline-terminated records count: a run killed now
    // keeps exactly those.
    let saved = std::fs::read_to_string(session.join("session.jsonl")).ok()?;
    saved
        .split_inclusive('\n')
        .any(|record| record.ends_with('\n') && record.contains(marker))
        .then(|| AttemptSave {
            run_id: run.file_name().into_string().unwrap(),
            attempt,
            session_id,
            session,
        })
}

fn rho(root: &TempDir, mode: &str, args: &[&str]) -> Output {
    command(root, mode).args(args).output().unwrap()
}

fn command(root: &TempDir, mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("RHO_HOME", root.path().join(".rho"))
        // Keep credential probes off the developer's session bus as well as HOME.
        .env("XDG_RUNTIME_DIR", root.path())
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env(MODE_ENV, mode)
        .env_remove(COMMAND_ENV)
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_PANE_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg("--config")
        .arg(root.path().join("config.toml"));
    command
}

fn success(output: &Output) -> &Output {
    assert!(
        output.status.success(),
        "rho failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
