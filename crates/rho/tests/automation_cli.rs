use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

use serde_json::Value;
use tempfile::TempDir;

const MODE_ENV: &str = "RHO_AUTOMATION_TEST_MODE";
const RESPONSE_ENV: &str = "RHO_AUTOMATION_TEST_RESPONSE";
const COMMAND_ENV: &str = "RHO_AUTOMATION_TEST_COMMAND";

#[test]
fn composes_prompt_arguments_stdin_and_combined_input() {
    let root = TempDir::new().unwrap();

    let arguments = run(&root, "inspect", &["run", "review", "this"], None);
    assert_success(&arguments);
    assert_eq!(user_prompt(&arguments), "review this");

    let stdin = run(&root, "inspect", &["run", "--stdin"], Some("diff contents"));
    assert_success(&stdin);
    assert_eq!(user_prompt(&stdin), "diff contents");

    let combined = run(
        &root,
        "inspect",
        &["run", "--stdin", "review"],
        Some("diff contents\n"),
    );
    assert_success(&combined);
    assert_eq!(user_prompt(&combined), "review\n\ndiff contents");
}

#[test]
fn applies_runtime_configuration_tools_and_workspace_instructions() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("AGENTS.md"), "project automation rules").unwrap();
    std::fs::write(
        root.path().join("config.toml"),
        r#"provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"
reasoning = "high"
web_search_provider = "disabled"
"#,
    )
    .unwrap();

    let output = run(
        &root,
        "inspect",
        &[
            "--auth",
            "xai-oauth",
            "--reasoning",
            "low",
            "run",
            "inspect",
        ],
        None,
    );
    assert_success(&output);
    let inspection = inspection(&output);

    assert_eq!(inspection["identity"]["provider"], "xai");
    assert_eq!(inspection["identity"]["model"], "grok-fixture");
    assert_eq!(inspection["reasoning"], "low");
    let system = inspection["messages"][0]["System"].as_str().unwrap();
    assert!(system.contains("project automation rules"));
    assert!(system.contains(&root.path().join("AGENTS.md").display().to_string()));

    let names = inspection["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    for expected in [
        "list_dir",
        "read_file",
        "write_file",
        "edit_file",
        "process",
        shell_tool_name(),
        "skill",
        "rho",
        "fetch_content",
        "get_search_content",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }
    assert!(!names.contains(&"web_search"));

    let config = std::fs::read_to_string(root.path().join("config.toml")).unwrap();
    assert!(config.contains("provider = \"xai\""));
    assert!(config.contains("model = \"grok-fixture\""));
    assert!(config.contains("auth = \"xai-oauth\""));
    assert!(config.contains("reasoning = \"low\""));
}

#[test]
fn applies_configured_tool_output_limit() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("large.txt"), "abcdefgh").unwrap();
    std::fs::write(
        root.path().join("config.toml"),
        "max_output_bytes = 5\nweb_search_provider = \"disabled\"\n",
    )
    .unwrap();

    let output = run(&root, "read-file", &["run", "read the file"], None);

    assert_success(&output);
    assert_eq!(stdout(&output), "abcde\n[truncated]\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn no_system_prompt_and_no_tools_only_affect_the_current_run() {
    let root = TempDir::new().unwrap();
    let output = run(
        &root,
        "inspect",
        &["--no-system-prompt", "--no-tools", "run", "hello"],
        None,
    );
    assert_success(&output);
    let inspection = inspection(&output);

    assert_eq!(inspection["messages"].as_array().unwrap().len(), 1);
    assert_eq!(user_prompt(&output), "hello");
    assert!(inspection["tools"].as_array().unwrap().is_empty());

    let config = std::fs::read_to_string(root.path().join("config.toml")).unwrap();
    assert!(!config.contains("no_system_prompt"));
    assert!(!config.contains("no_tools"));
}

#[test]
fn provider_and_tool_failures_stay_off_stdout() {
    let root = TempDir::new().unwrap();
    let provider_failure = run(&root, "fail", &["run", "hello"], None);
    assert_eq!(provider_failure.status.code(), Some(1));
    assert!(provider_failure.stdout.is_empty());
    assert!(stderr(&provider_failure).contains("deterministic provider failure"));

    let mut command = command(&root, "tool-failure");
    command
        .env(RESPONSE_ENV, "recovered after tool failure")
        .args(["run", "use a tool"]);
    let tool_failure = command.output().unwrap();
    assert_success(&tool_failure);
    assert_eq!(stdout(&tool_failure), "recovered after tool failure\n");
    assert!(tool_failure.stderr.is_empty());
}

#[test]
fn jsonl_success_emits_versioned_events_and_authoritative_text() {
    let root = TempDir::new().unwrap();
    let mut command = command(&root, "fixed");
    command
        .env(RESPONSE_ENV, "answer for automation")
        .args(["run", "--output", "jsonl", "hello"]);
    let output = command.output().unwrap();

    assert_success(&output);
    assert!(output.stderr.is_empty());
    let events = jsonl_events(&output);
    assert_eq!(
        events
            .iter()
            .map(|event| event["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        (1..=events.len() as u64).collect::<Vec<_>>()
    );
    assert!(events.iter().all(|event| event["schema_version"] == 1));
    assert_eq!(events.first().unwrap()["type"], "run.started");
    assert!(same_file::is_same_file(
        events.first().unwrap()["workspace"].as_str().unwrap(),
        root.path(),
    )
    .unwrap());
    assert!(events.iter().any(|event| {
        event["type"] == "assistant.text_delta"
            && event["attempt"] == 1
            && event["text"] == "answer for automation"
    }));
    assert_eq!(
        events.last().unwrap(),
        &serde_json::json!({
            "schema_version": 1,
            "seq": events.len(),
            "type": "run.completed",
            "reason": "completed",
            "text": "answer for automation"
        })
    );
}

#[test]
fn jsonl_tracks_provider_retries_without_exposing_diagnostics() {
    let root = TempDir::new().unwrap();
    let mut command = command(&root, "retry");
    command
        .env(RESPONSE_ENV, "recovered")
        .args(["run", "--output", "jsonl", "hello"]);
    let output = command.output().unwrap();

    assert_success(&output);
    let events = jsonl_events(&output);
    assert!(events
        .iter()
        .any(|event| { event["type"] == "assistant.text_reset" && event["attempt"] == 1 }));
    assert!(events.iter().any(|event| {
        event["type"] == "assistant.text_delta"
            && event["attempt"] == 2
            && event["text"] == "recovered"
    }));
    assert!(!stdout(&output).contains("deterministic retryable failure"));
}

#[test]
fn jsonl_reports_recoverable_tool_failure_and_run_completion() {
    let root = TempDir::new().unwrap();
    let mut command = command(&root, "tool-failure");
    command
        .env(RESPONSE_ENV, "recovered after tool failure")
        .args(["run", "--output", "jsonl", "use a tool"]);
    let output = command.output().unwrap();

    assert_success(&output);
    let events = jsonl_events(&output);
    assert!(events.iter().any(|event| {
        event["type"] == "tool.started"
            && event["call_id"] == "fixture-tool-failure"
            && event["name"] == "read_file"
    }));
    assert!(events.iter().any(|event| {
        event["type"] == "tool.finished"
            && event["call_id"] == "fixture-tool-failure"
            && event["status"] == "failure"
    }));
    assert_eq!(events.last().unwrap()["type"], "run.completed");
    assert!(!stdout(&output).contains("outside-workspace"));
}

#[test]
fn jsonl_reports_provider_failure_as_the_only_terminal_event() {
    let root = TempDir::new().unwrap();
    let output = run(&root, "fail", &["run", "--output", "jsonl", "hello"], None);

    assert_eq!(output.status.code(), Some(1));
    let events = jsonl_events(&output);
    let terminals = events
        .iter()
        .filter(|event| {
            matches!(
                event["type"].as_str(),
                Some("run.completed" | "run.failed" | "run.stopped")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["type"], "run.failed");
    assert_eq!(terminals[0]["reason"], "provider_error");
    assert_eq!(events.last().unwrap(), terminals[0]);
}

#[test]
fn jsonl_sanitizes_authentication_failures() {
    let root = TempDir::new().unwrap();
    let output = run(
        &root,
        "auth-failure",
        &["run", "--output", "jsonl", "hello"],
        None,
    );

    assert_eq!(output.status.code(), Some(1));
    let events = jsonl_events(&output);
    assert_eq!(events.last().unwrap()["type"], "run.failed");
    assert_eq!(events.last().unwrap()["reason"], "authentication");
    assert_eq!(events.last().unwrap()["message"], "authentication failed");
    assert!(!stdout(&output).contains("fixture-secret"));
    assert!(!stderr(&output).contains("fixture-secret"));
}

#[test]
fn explicit_step_limit_stops_with_status_124() {
    let root = TempDir::new().unwrap();
    let output_file = root.path().join("result.json");
    let output = run(
        &root,
        "tool-failure",
        &[
            "run",
            "--output",
            "jsonl",
            "--output-file",
            output_file.to_str().unwrap(),
            "--max-steps",
            "1",
            "use a tool",
        ],
        None,
    );

    assert_eq!(output.status.code(), Some(124));
    let events = jsonl_events(&output);
    assert_eq!(events.last().unwrap()["type"], "run.stopped");
    assert_eq!(events.last().unwrap()["reason"], "max_steps");
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(output_file).unwrap()).unwrap();
    assert_eq!(result["state"], "stopped");
}

#[test]
fn timeout_stops_with_status_124() {
    let root = TempDir::new().unwrap();
    let output = run(
        &root,
        "delay",
        &["run", "--output", "jsonl", "--timeout", "50ms", "wait"],
        None,
    );

    assert_eq!(output.status.code(), Some(124));
    let events = jsonl_events(&output);
    assert_eq!(events.last().unwrap()["type"], "run.stopped");
    assert_eq!(events.last().unwrap()["reason"], "timeout");
}

#[test]
fn startup_failure_emits_a_terminal_json_object() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("config.toml"), "not valid toml = [").unwrap();
    let output = run(&root, "fixed", &["run", "--output", "jsonl", "hello"], None);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        jsonl_events(&output),
        vec![serde_json::json!({
            "schema_version": 1,
            "seq": 1,
            "type": "run.failed",
            "reason": "configuration_error",
            "message": "configuration failed"
        })]
    );
}

#[test]
fn broken_jsonl_stdout_fails_the_run() {
    let root = TempDir::new().unwrap();
    let mut command = command(&root, "fixed");
    command.args(["run", "--output", "jsonl", "hello"]);
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    command.stdout(writer);

    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("output failed"));
}

#[test]
fn output_run_persists_agent_identity() {
    let root = TempDir::new().unwrap();
    std::fs::write(
        root.path().join("config.toml"),
        "provider = \"openai\"\nmodel = \"gpt-5.5\"\n",
    )
    .unwrap();
    let output_file = root.path().join("result.json");
    let mut command = command(&root, "fixed");
    command.env(RESPONSE_ENV, "done").args([
        "--no-subagents",
        "--agent",
        "worker",
        "run",
        "--output-file",
        output_file.to_str().unwrap(),
        "complete the task",
    ]);
    let output = command.output().unwrap();
    assert_success(&output);

    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(&output_file).unwrap()).unwrap();
    assert_eq!(result["state"], "ok");
    assert_eq!(result["agent_id"], "worker");
    assert_eq!(result["agent_fingerprint"].as_str().unwrap().len(), 64);
    assert_eq!(result["provider"], "openai");
    assert!(result["model"]
        .as_str()
        .is_some_and(|model| !model.is_empty()));
    let events = std::fs::read_to_string(root.path().join("events.jsonl")).unwrap();
    assert!(events.contains("complete the task"));
}

#[test]
fn failed_output_run_persists_resolved_provider_and_model() {
    let root = TempDir::new().unwrap();
    let output_file = root.path().join("result.json");
    let mut command = command(&root, "fail");
    command.args([
        "--no-subagents",
        "run",
        "--output-file",
        output_file.to_str().unwrap(),
        "complete the task",
    ]);
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));

    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(&output_file).unwrap()).unwrap();
    assert_eq!(result["state"], "error");
    assert_eq!(result["provider"], "openai");
    assert_eq!(result["model"], "gpt-5.5");
    assert!(result["error"]
        .as_str()
        .unwrap()
        .contains("deterministic provider failure"));
}

#[test]
fn final_answer_is_the_only_stdout_content() {
    let root = TempDir::new().unwrap();
    let mut command = command(&root, "fixed");
    command
        .env(RESPONSE_ENV, "answer for a pipeline")
        .args(["run", "hello"]);
    let output = command.output().unwrap();

    assert_success(&output);
    assert_eq!(stdout(&output), "answer for a pipeline\n");
    assert!(output.stderr.is_empty());

    let ledger = rusqlite::Connection::open(root.path().join(".rho/usage.sqlite3")).unwrap();
    let request: (String, String, String) = ledger
        .query_row(
            "SELECT provider, model, purpose FROM usage_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(request, ("openai".into(), "gpt-5.5".into(), "agent".into()));
}

#[cfg(unix)]
#[test]
fn jsonl_sigterm_emits_stopped_event_after_cleanup() {
    use std::io::{BufRead, Read};

    let root = TempDir::new().unwrap();
    let mut command = command(&root, "delay");
    command.args(["run", "--output", "jsonl", "wait"]);
    let mut child = command.spawn().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut stream = String::new();
    stdout.read_line(&mut stream).unwrap();
    assert!(stream.contains("\"type\":\"run.started\""));

    let signal_status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal_status.success());
    stdout.read_to_string(&mut stream).unwrap();
    let status = child.wait().unwrap();

    assert_eq!(status.code(), Some(143));
    let events = stream
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.last().unwrap()["type"], "run.stopped");
    assert_eq!(events.last().unwrap()["reason"], "interrupted");
}

#[cfg(unix)]
#[test]
fn interrupt_reports_herdr_lifecycle_and_cleans_up_background_processes() {
    use std::{
        os::unix::net::UnixListener,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    let root = TempDir::new().unwrap();
    let ready = root.path().join("process-ready");
    let leaked = root.path().join("process-leaked");
    let socket = root.path().join("herdr.sock");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = Arc::clone(&requests);
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
            server_requests
                .lock()
                .unwrap()
                .push(serde_json::from_str::<Value>(&line).unwrap());
            stream.write_all(b"{}\n").unwrap();
        }
    });

    let process_command = format!(
        "printf started > '{}'; sleep 5; printf leaked > '{}'",
        ready.display(),
        leaked.display()
    );
    let mut command = command(&root, "process-then-delay");
    command
        .env(COMMAND_ENV, process_command)
        .env("HERDR_ENV", "1")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PANE_ID", "%fixture")
        .args(["run", "start background work"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ready.exists(), "fixture process did not start");
    let signal_status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal_status.success());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("interrupted by SIGINT"));

    server.join().unwrap();
    let methods = requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request["method"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        [
            "pane.report_agent",
            "pane.report_agent",
            "pane.release_agent"
        ]
    );
    let states = requests
        .lock()
        .unwrap()
        .iter()
        .filter_map(|request| request["params"]["state"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    assert_eq!(states, ["working", "idle"]);

    std::thread::sleep(Duration::from_millis(300));
    assert!(!leaked.exists(), "background process survived rho shutdown");
}

fn run(root: &TempDir, mode: &str, args: &[&str], input: Option<&str>) -> Output {
    let mut command = command(root, mode);
    command.args(args);
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

/// Builds a configured command for running the `rho` binary in a temporary workspace.
///
/// The command uses the workspace as its current directory and home, selects the
/// specified fixture mode, removes environment variables that could affect test
/// isolation, pipes standard output and error, and loads the workspace
/// configuration file.
///
/// # Arguments
///
/// * `root` - Temporary workspace used as the command's working directory and home.
/// * `mode` - Fixture mode passed through the automation test environment variable.
///
/// # Examples
///
/// ```
/// let root = tempfile::tempdir().unwrap();
/// let command = command(&root, "fixed");
///
/// assert!(command.get_args().any(|arg| arg == "--config"));
/// ```
fn command(root: &TempDir, mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("RHO_HOME", root.path().join(".rho"))
        .env(MODE_ENV, mode)
        .env_remove(RESPONSE_ENV)
        .env_remove(COMMAND_ENV)
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_PANE_ID")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg("--config")
        .arg(root.path().join("config.toml"));
    command
}

/// Parses each line of command output as a JSON value.
///
/// # Panics
///
/// Panics if any output line is not valid JSON.
///
/// # Examples
///
/// ```
/// let events = jsonl_events(&output);
/// assert_eq!(events[0]["type"], "run.started");
/// ```
fn jsonl_events(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn inspection(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim()).unwrap()
}

fn user_prompt(output: &Output) -> String {
    let inspection = inspection(output);
    inspection["messages"].as_array().unwrap().last().unwrap()["User"][0]["Text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {}\nstdout: {}\nstderr: {}",
        output.status,
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).unwrap()
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).unwrap()
}

fn shell_tool_name() -> &'static str {
    if cfg!(windows) {
        "powershell"
    } else {
        "bash"
    }
}
