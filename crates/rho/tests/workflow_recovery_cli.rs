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
/// The whole test, both runs included, measured 0.19 s locally. Giving up
/// before the fixture's own delay ends keeps a missing checkpoint a clear
/// failure instead of a run that quietly completes.
const CHECKPOINT_DEADLINE: Duration = Duration::from_secs(25);

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

// Covers: a Rho agent node killed mid-run continues the same attempt from its
// step checkpoint, so the model sees the saved history (including the finished
// tool call) instead of a fresh prompt, and `--dry-run` predicts that.
// Owner: workflow crash recovery (SDK checkpoint store wired through the
// headless agent runner).
#[test]
fn killed_agent_node_continues_from_its_checkpoint() {
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

    // Turn one starts a process; turn two hangs, so the run is killed after the
    // checkpoint that holds the finished tool call.
    let mut first = command(&root, "process-then-delay");
    first
        .env(COMMAND_ENV, "true")
        .args(["workflow", "run", plan_id, "--yes", "--output", "jsonl"]);
    let mut first = first.spawn().unwrap();
    let (run_id, checkpoint) = wait_for_tool_checkpoint(&root);
    first.kill().unwrap();
    first.wait().unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&checkpoint).unwrap()).unwrap();

    let preview = rho(
        &root,
        "inspect",
        &[
            "workflow",
            "resume",
            &run_id,
            "--dry-run",
            "--output",
            "jsonl",
        ],
    );
    let preview: Value = serde_json::from_slice(&success(&preview).stdout).unwrap();
    assert_eq!(
        preview,
        json!({
            "run_id": run_id,
            "needs_confirmation": true,
            "attempts": [{"node": "inspect", "attempt": 1, "action": "continue"}],
        })
    );

    let resumed = rho(
        &root,
        "inspect",
        &[
            "workflow",
            "resume",
            &run_id,
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
        ]
    );
    // The inspect fixture answers with the request it received.
    let answer = checkpoint.with_file_name("answer.txt");
    let request: Value = serde_json::from_slice(&std::fs::read(answer).unwrap()).unwrap();
    assert_eq!(request["messages"], saved["history"]);
    assert!(
        !checkpoint.exists(),
        "finished attempts drop their checkpoint"
    );
}

/// Waits until attempt 1 has saved a checkpoint holding the user prompt, the
/// tool call, and its result. Returns the run ID and the checkpoint path.
fn wait_for_tool_checkpoint(root: &TempDir) -> (String, PathBuf) {
    let runs = root.path().join(".rho/workflows/runs");
    let started = Instant::now();
    loop {
        if let Some(found) = tool_checkpoint(&runs) {
            return found;
        }
        assert!(
            started.elapsed() < CHECKPOINT_DEADLINE,
            "no tool checkpoint within {CHECKPOINT_DEADLINE:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn tool_checkpoint(runs: &Path) -> Option<(String, PathBuf)> {
    let run = std::fs::read_dir(runs).ok()?.next()?.ok()?;
    let checkpoint = run
        .path()
        .join("nodes/inspect/attempts/1/agent/session.json");
    // Saves replace the file atomically, so any read sees a complete snapshot.
    let snapshot: Value = serde_json::from_slice(&std::fs::read(&checkpoint).ok()?).ok()?;
    let has_tool_result = snapshot["history"]
        .as_array()?
        .iter()
        .any(|message| message.get("ToolResult").is_some());
    has_tool_result.then(|| (run.file_name().into_string().unwrap(), checkpoint))
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
