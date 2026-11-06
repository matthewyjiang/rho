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
/// The `process-then-delay` fixture holds its second model request for 30 s.
/// One row of the test, both runs included, measured 0.19 s locally (with the
/// attempt checkpoint this replaced; the parent re-measures). Giving up
/// before the fixture's own delay ends keeps a missing save a clear failure
/// instead of a run that quietly completes.
const SAVE_DEADLINE: Duration = Duration::from_secs(25);

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
}

// Covers: a Rho agent node killed mid-run continues the same attempt by
// reopening the session it recorded, so the model sees the saved history
// (including the finished tool call) and not the prompt a second time, or
// only the prompt when nothing was saved; `--dry-run` predicts that.
// Owner: workflow crash recovery (the session id the attempt records,
// reopened through the headless agent runner).
#[test]
fn killed_agent_node_continues_its_saved_session() {
    for (saved, expected) in [
        // (user messages, tool results) in the resumed model request.
        (SavedSession::Kept, (1, 1)),
        (SavedSession::Removed, (1, 0)),
    ] {
        let root = TempDir::new().unwrap();
        std::fs::create_dir_all(root.path().join("wf/agents")).unwrap();
        std::fs::write(root.path().join("config.toml"), CONFIG).unwrap();
        std::fs::write(root.path().join("wf/main.star"), WORKFLOW).unwrap();
        std::fs::write(root.path().join("wf/agents/fixture.md"), AGENT).unwrap();

        let plan = rho(
            &root,
            "inspect",
            &["workflow", "plan", "wf/main.star", "--output", "json"],
        );
        let plan: Value = serde_json::from_slice(&success(&plan).stdout).unwrap();
        let plan_id = plan["manifest"]["plan_id"].as_str().unwrap();

        // Turn one starts a process; turn two hangs, so the run is killed
        // after the save that holds the finished tool call.
        let mut first = command(&root, "process-then-delay");
        first
            .env(COMMAND_ENV, "true")
            .args(["workflow", "run", plan_id, "--yes", "--output", "jsonl"]);
        let mut first = first.spawn().unwrap();
        let save = wait_for_tool_result_save(&root);
        first.kill().unwrap();
        first.wait().unwrap();
        match saved {
            SavedSession::Kept => {}
            SavedSession::Removed => std::fs::remove_dir_all(&save.session).unwrap(),
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
                "attempts": [{"node": "s0.inspect", "attempt": 1, "action": "continue"}],
            }),
            "{saved:?}"
        );

        let resumed = rho(
            &root,
            "inspect",
            &[
                "workflow",
                "resume",
                &save.run_id,
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
        assert_eq!(
            node_events,
            [
                json!(["node_started", 1, null]),
                json!(["node_finished", null, "success"]),
            ],
            "{saved:?}"
        );
        // The inspect fixture answers with the request it received.
        let answer = save.attempt.join("agent/answer.txt");
        let request: Value = serde_json::from_slice(&std::fs::read(answer).unwrap()).unwrap();
        let messages = request["messages"].as_array().unwrap();
        let count = |kind: &str| {
            messages
                .iter()
                .filter(|message| message.get(kind).is_some())
                .count()
        };
        assert_eq!((count("User"), count("ToolResult")), expected, "{saved:?}");
    }
}

/// Where attempt 1 of a run saved its agent's session.
struct AttemptSave {
    run_id: String,
    attempt: PathBuf,
    /// The session's directory below its workspace's.
    session: PathBuf,
}

/// Waits until attempt 1's agent has saved its session with the tool call's
/// result.
fn wait_for_tool_result_save(root: &TempDir) -> AttemptSave {
    let started = Instant::now();
    loop {
        if let Some(found) = tool_result_save(root.path()) {
            return found;
        }
        assert!(
            started.elapsed() < SAVE_DEADLINE,
            "no tool result saved within {SAVE_DEADLINE:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn tool_result_save(root: &Path) -> Option<AttemptSave> {
    let run = std::fs::read_dir(root.join(".rho/workflows/runs"))
        .ok()?
        .next()?
        .ok()?;
    let attempt = run.path().join("nodes/s0.inspect/attempts/1");
    let record: Value =
        serde_json::from_slice(&std::fs::read(attempt.join("status.json")).ok()?).ok()?;
    let id = record["state"]["agent_session"].as_str()?;
    // Each session is a directory named for its id, below its workspace's.
    let suffix = format!("_{id}");
    let session = std::fs::read_dir(root.join(".rho/sessions"))
        .ok()?
        .flatten()
        .filter_map(|workspace| std::fs::read_dir(workspace.path()).ok())
        .flatten()
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))?
        .path();
    // Every message a save writes is tagged with its kind.
    let saved = std::fs::read_to_string(session.join("session.jsonl")).ok()?;
    saved.contains("\"ToolResult\"").then(|| AttemptSave {
        run_id: run.file_name().into_string().unwrap(),
        attempt,
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
