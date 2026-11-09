use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

use rho_tools::compact_display_path;
use serde_json::{json, Value};
use tempfile::TempDir;

const MODE_ENV: &str = "RHO_AUTOMATION_TEST_MODE";
const RESPONSE_ENV: &str = "RHO_AUTOMATION_TEST_RESPONSE";
const PATH_ENV: &str = "RHO_AUTOMATION_TEST_PATH";

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
        r#"[model]
provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"
reasoning = "high"

[web_search]
mode = "off"
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
    // Assembly injects AGENTS.md paths as JSON path-data attributes. Do not
    // require TempDir's lexical path: macOS current_dir is often the
    // /private/var form while TempDir reports /var.
    assert!(
        system.lines().any(|line| {
            line.contains("<agents_instructions path=") && line.contains("AGENTS.md")
        }),
        "expected AGENTS.md path data on agents_instructions open tag"
    );

    let names = inspection["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    for expected in [
        "list_dir",
        "read_file",
        "write",
        // Auto edit preference for xai resolves to str_replace.
        "str_replace",
        "grep",
        "glob",
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
    assert!(!names.contains(&"edit"));
    assert!(!names.contains(&"apply_patch"));

    let config = std::fs::read_to_string(root.path().join("config.toml")).unwrap();
    // CLI overrides are session-only unless --save is set.
    assert!(config.contains("reasoning = \"high\""));
    assert!(!config.contains("reasoning = \"low\""));

    let saved = run(
        &root,
        "inspect",
        &[
            "--auth",
            "xai-oauth",
            "--reasoning",
            "low",
            "--save",
            "run",
            "inspect",
        ],
        None,
    );
    assert_success(&saved);
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
        "web_search.mode = \"off\"\n\n[output]\nmax_output_bytes = 5\n",
    )
    .unwrap();

    let output = run(&root, "read-file", &["run", "read the file"], None);

    assert_success(&output);
    // The path header is emitted before any content line, so a 5-byte budget
    // cuts inside the header itself.
    assert_eq!(stdout(&output), "large\n[truncated]\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn reads_and_writes_paths_outside_the_working_directory() {
    let root = TempDir::new().unwrap();
    let outside = tempfile::tempdir_in(root.path().parent().unwrap()).unwrap();
    let input_path = outside.path().join("input.txt");
    std::fs::write(&input_path, "outside content").unwrap();
    let relative_input_path = std::path::Path::new("..")
        .join(outside.path().file_name().unwrap())
        .join("input.txt");

    let mut read = command(&root, "read-path");
    read.env(PATH_ENV, &relative_input_path)
        .args(["run", "read the outside file"]);
    let read_output = read.output().unwrap();
    assert_success(&read_output);
    // Tools display paths relative to the canonical workspace root (macOS /var
    // -> /private/var, Windows short paths / \\?\ prefixes), so match that here.
    let workspace_root = root.path().canonicalize().unwrap();
    let display = compact_display_path(&workspace_root, &relative_input_path.to_string_lossy());
    let read_stdout = stdout(&read_output);
    let (header, body) = read_stdout.trim_end().split_once('\n').unwrap();
    assert_eq!(header, display);
    assert_eq!(body, "1:outside content");

    let output_path = outside.path().join("output.txt");
    let mut write = command(&root, "write-path");
    write
        .env(PATH_ENV, &output_path)
        .args(["run", "write the outside file"]);
    let write_output = write.output().unwrap();
    assert_success(&write_output);
    assert_eq!(
        std::fs::read_to_string(output_path).unwrap(),
        "written outside the working directory"
    );
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

    // Reading config never writes it, so a run that saves nothing leaves
    // no file at all.
    let config = std::fs::read_to_string(root.path().join("config.toml")).unwrap_or_default();
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
    assert!(stderr(&provider_failure).contains("provider diagnostic: fixture provider diagnostic"));

    let mut command = command(&root, "tool-failure");
    command
        .env(RESPONSE_ENV, "recovered after tool failure")
        .args(["run", "use a tool"]);
    let tool_failure = command.output().unwrap();
    assert_success(&tool_failure);
    assert_eq!(stdout(&tool_failure), "recovered after tool failure\n");
    assert!(tool_failure.stderr.is_empty());
}

// Covers: the JSONL stream is schema 2: numbered lines from `run_started`,
// naming the session and workspace, through the daemon protocol's session
// events, the turn's `turn_ended` with the whole answer, and `run_ended`
// (#1669).
// Owner: rho run JSONL contract (process contract)
#[test]
fn jsonl_streams_the_sessions_events_between_start_and_end() {
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
            .map(|event| (event["schema_version"].as_u64(), event["seq"].as_u64()))
            .collect::<Vec<_>>(),
        (1..=events.len() as u64)
            .map(|seq| (Some(2), Some(seq)))
            .collect::<Vec<_>>()
    );
    let started = &events[0];
    assert_eq!(started["type"], "run_started");
    assert!(started["session"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(same_file::is_same_file(started["workspace"].as_str().unwrap(), root.path()).unwrap());
    assert_eq!(
        session_events(&events)
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .filter(|kind| *kind != "usage")
            .collect::<Vec<_>>(),
        ["user_message", "agent_message_chunk", "turn_ended"]
    );
    assert_eq!(
        &events[events.len() - 2..],
        [
            json!({
                "schema_version": 2,
                "seq": events.len() - 1,
                "type": "event",
                "event": {
                    "type": "turn_ended",
                    "outcome": {"type": "answered", "text": "answer for automation", "stop": "end_turn"}
                }
            }),
            json!({
                "schema_version": 2,
                "seq": events.len(),
                "type": "run_ended",
                "reason": "completed",
                "exit_code": 0
            }),
        ]
    );
}

// Covers: a provider retry shows as a discarded response, and the turn's
// answer holds only the retried response's text; the provider's error stays
// out.
// Owner: rho run JSONL contract (process contract)
#[test]
fn jsonl_tracks_provider_retries_without_exposing_diagnostics() {
    let root = TempDir::new().unwrap();
    let mut command = command(&root, "retry");
    command
        .env(RESPONSE_ENV, "recovered")
        .args(["run", "--output", "jsonl", "hello"]);
    let output = command.output().unwrap();

    assert_success(&output);
    let lines = jsonl_events(&output);
    let events = session_events(&lines);
    assert!(events
        .iter()
        .any(|event| event["type"] == "response_discarded"));
    assert_eq!(
        events.last().unwrap()["outcome"]["text"],
        "recovered",
        "{events:#?}"
    );
    assert!(!stdout(&output).contains("deterministic retryable failure"));
}

// Covers: a tool call that fails shows as a failed call, and a model that
// recovers still completes the run.
// Owner: rho run JSONL contract (process contract)
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
    assert!(
        session_events(&events)
            .iter()
            .any(|event| event["type"] == "tool_call" && event["status"] == "failed"),
        "{events:#?}"
    );
    assert_eq!(run_ended(&events), ("completed", 0));
}

// Covers: a failed turn ends with its failure's kind and message, then one
// `run_ended`; an authentication failure names only the provider, auth,
// and how to log in, never the provider's text; the provider's diagnostic
// goes to stderr only.
// Owner: rho run JSONL contract (process contract)
#[test]
fn jsonl_reports_a_failed_turn_and_hides_authentication_text() {
    let cases = [
        (
            "fail",
            "provider",
            "provider_error",
            "provider failed: deterministic provider failure",
        ),
        (
            "auth-failure",
            "authentication",
            "authentication",
            "authentication failed for openai (auth: api-key); run /login openai",
        ),
    ];
    for (mode, kind, reason, message) in cases {
        let root = TempDir::new().unwrap();
        let output = run(&root, mode, &["run", "--output", "jsonl", "hello"], None);

        assert_eq!(output.status.code(), Some(1), "{mode}");
        let events = jsonl_events(&output);
        assert_eq!(run_ended(&events), (reason, 1), "{mode}");
        assert_eq!(
            &events[events.len() - 2..],
            [
                json!({
                    "schema_version": 2,
                    "seq": events.len() - 1,
                    "type": "event",
                    "event": {
                        "type": "turn_ended",
                        "outcome": {"type": "failed", "error": {"kind": kind, "message": message}}
                    }
                }),
                json!({
                    "schema_version": 2,
                    "seq": events.len(),
                    "type": "run_ended",
                    "reason": reason,
                    "exit_code": 1,
                    "message": message
                }),
            ],
            "{mode}"
        );
        assert!(!stdout(&output).contains("fixture-secret"), "{mode}");
        assert!(!stderr(&output).contains("fixture-secret"), "{mode}");
        assert!(
            !stdout(&output).contains("fixture provider diagnostic"),
            "{mode}"
        );
        // Diagnostics are for the person at the terminal, never an
        // authentication failure's, which may echo credentials.
        assert_eq!(
            stderr(&output).contains("provider diagnostic: fixture provider diagnostic"),
            mode == "fail",
            "{mode}: {}",
            stderr(&output)
        );
    }
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
    assert_eq!(run_ended(&events), ("max_steps", 124));
    assert_eq!(
        session_events(&events).last().unwrap()["outcome"]["type"],
        "max_steps"
    );
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(output_file).unwrap()).unwrap();
    assert_eq!(result["state"], "stopped");
}

#[test]
fn step_limit_in_text_mode_exits_124() {
    let root = TempDir::new().unwrap();
    let output = run(
        &root,
        "tool-failure",
        &["run", "--max-steps", "1", "use a tool"],
        None,
    );

    assert_eq!(output.status.code(), Some(124));
    assert!(
        stderr(&output).contains("model-step limit"),
        "stderr: {}",
        stderr(&output)
    );
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
    assert_eq!(run_ended(&events), ("timeout", 124));
    assert_eq!(
        session_events(&events).last().unwrap()["outcome"]["type"],
        "cancelled"
    );
}

#[test]
fn startup_failure_writes_only_run_ended() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("config.toml"), "not valid toml = [").unwrap();
    let output = run(&root, "fixed", &["run", "--output", "jsonl", "hello"], None);

    assert_eq!(output.status.code(), Some(2));
    let events = jsonl_events(&output);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["schema_version"], 2);
    assert_eq!(events[0]["seq"], 1);
    assert_eq!(run_ended(&events), ("configuration_error", 2));
    let message = events[0]["message"].as_str().unwrap();
    assert_ne!(message, "configuration failed");
    assert!(
        !message.is_empty(),
        "configuration failure must carry parse detail"
    );
    assert!(
        stderr(&output).contains(message),
        "stderr should carry the same configuration detail"
    );
}

// Covers: a JSONL stream that cannot be written ends the run as an output
// error, exit 1, naming the write that failed whatever the run's own end:
// one that failed to start (else exit 2) or could not open its status file.
// Owner: rho run JSONL contract (process contract)
#[test]
fn broken_jsonl_stdout_fails_the_run() {
    let cases: [(&str, &str, &[&str]); 3] = [
        ("runs", "", &[]),
        ("fails to start", "not valid toml = [", &[]),
        // The workspace is a directory, so no status file can be written there.
        ("cannot keep its status file", "", &["--output-file", "."]),
    ];
    for (case, config, flags) in cases {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("config.toml"), config).unwrap();
        let mut command = command(&root, "fixed");
        command
            .args(["run", "--output", "jsonl"])
            .args(flags)
            .arg("hello");
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        command.stdout(writer);

        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{case}");
        assert!(
            stderr(&output).contains("could not write JSONL output"),
            "{case}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn redirected_stdin_without_flag_is_an_error() {
    let root = TempDir::new().unwrap();
    let output = run(
        &root,
        "fixed",
        &["run", "review this"],
        Some("this diff would be lost"),
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("--stdin"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("this diff would be lost"));
}

#[test]
fn output_file_text_mode_streams_answer_and_ends_with_completion_marker() {
    let root = TempDir::new().unwrap();
    let output_file = root.path().join("result.json");
    let mut command = command(&root, "fixed");
    command
        .env(RESPONSE_ENV, "answer stays in the result file")
        .args([
            "run",
            "--output-file",
            output_file.to_str().unwrap(),
            "complete the task",
        ]);
    let output = command.output().unwrap();
    assert_success(&output);
    let stdout = stdout(&output);
    assert!(
        stdout.contains("answer stays in the result file"),
        "stdout streams assistant text for live watching: {stdout}"
    );
    assert!(
        stdout.contains("[subagent run complete]"),
        "stdout: {stdout}"
    );
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(&output_file).unwrap()).unwrap();
    assert_eq!(result["state"], "ok");
    assert_eq!(result["result"], "answer stays in the result file");
}

#[test]
fn output_run_persists_agent_identity() {
    let root = TempDir::new().unwrap();
    std::fs::write(
        root.path().join("config.toml"),
        "[model]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\n",
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

    // The agent's request is the run's only one: no title-agent call.
    let ledger = rusqlite::Connection::open(root.path().join(".rho/usage.sqlite3")).unwrap();
    let requests = ledger
        .prepare("SELECT provider, model, purpose FROM usage_events")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<(String, String, String)>>>()
        .unwrap();
    assert_eq!(
        requests,
        vec![("openai".into(), "gpt-5.5".into(), "agent".into())]
    );
}

// Covers: `rho run` saves its session under the session root, recorded as
// a run's; `rho sessions list` leaves it out, and it opens by id.
// `--no-save` leaves no session behind to list or open.
// Owner: rho run session storage (process contract)
#[test]
fn run_saves_its_session_as_a_hidden_run_unless_told_not_to() {
    // (run flags, saved header origins, opens by id)
    let cases: [(&[&str], Vec<Value>, bool); 2] = [
        (&[], vec![Value::from("run")], true),
        (&["--no-save"], Vec::new(), false),
    ];
    for (flags, origins, opens) in cases {
        let root = TempDir::new().unwrap();
        let mut run = command(&root, "fixed");
        run.env(RESPONSE_ENV, "saved answer")
            .args(["run", "--output", "jsonl"])
            .args(flags)
            .arg("remember this");
        let output = run.output().unwrap();
        assert_success(&output);
        let events = jsonl_events(&output);
        let session_id = events.first().unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned();

        let mut list = command(&root, "fixed");
        list.args(["sessions", "list", "--json"]);
        let listed = list.output().unwrap();
        assert_success(&listed);
        let listing = inspection(&listed);
        let exported = root.path().join("run-transcript.json");
        let mut export = command(&root, "fixed");
        export
            .args(["sessions", "export", &session_id, "--output"])
            .arg(&exported);
        let opened = export.output().unwrap().status.success().then(|| {
            let transcript: Value =
                serde_json::from_str(&std::fs::read_to_string(&exported).unwrap()).unwrap();
            (
                transcript["id"].clone(),
                transcript["messages"][0]["message"]["User"][0]["Text"].clone(),
            )
        });
        assert_eq!(
            (
                listing["sessions"].as_array().unwrap().len(),
                saved_session_headers(&root, &session_id)
                    .into_iter()
                    .map(|header| header["origin"].clone())
                    .collect::<Vec<_>>(),
                opened,
            ),
            (
                0,
                origins,
                opens.then(|| (Value::from(session_id), Value::from("remember this"))),
            ),
            "{flags:?}"
        );
    }
}

/// The headers of the sessions saved under `root` as `session_id`.
fn saved_session_headers(root: &TempDir, session_id: &str) -> Vec<Value> {
    let sessions = root.path().join(".rho/sessions");
    if !sessions.is_dir() {
        return Vec::new();
    }
    std::fs::read_dir(sessions)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .flat_map(|workspace| std::fs::read_dir(workspace).unwrap())
        .map(|session| session.unwrap().path())
        .filter(|session| {
            session
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&format!("_{session_id}")))
        })
        .map(|session| {
            let transcript = std::fs::read_to_string(session.join("session.jsonl")).unwrap();
            let header = transcript.lines().next().unwrap().to_owned();
            serde_json::from_str::<Value>(&header).unwrap()
        })
        .collect()
}

// Covers: `rho run` resolves the slash commands it runs, such as a prompt
// template, and rejects the ones it cannot run as configuration errors
// before any model call.
// Owner: rho run command resolution (process contract)
#[test]
fn resolves_slash_commands_and_rejects_the_ones_it_cannot_run() {
    let root = TempDir::new().unwrap();
    let prompts = root.path().join(".rho/prompts");
    std::fs::create_dir_all(&prompts).unwrap();
    std::fs::write(prompts.join("review.md"), "Review $ARGUMENTS carefully").unwrap();

    let expanded = run(&root, "inspect", &["run", "/prompt:review the diff"], None);
    assert_success(&expanded);
    assert_eq!(user_prompt(&expanded), "Review the diff carefully");

    for prompt in ["/compact", "/compact keep the plan"] {
        let rejected = run(
            &root,
            "inspect",
            &["run", "--output", "jsonl", prompt],
            None,
        );
        assert_eq!(rejected.status.code(), Some(2), "{prompt}");
        let events = jsonl_events(&rejected);
        assert_eq!(run_ended(&events), ("configuration_error", 2), "{prompt}");
        // The prompt ends as the daemon's would, with no model call.
        let turn_ended = json!({
            "type": "turn_ended",
            "outcome": {"type": "failed", "error": {"kind": "configuration", "message": "could not run /compact: not available in rho run"}}
        });
        assert_eq!(session_events(&events), [&turn_ended], "{prompt}");
    }
}

#[cfg(unix)]
#[test]
fn jsonl_sigterm_ends_the_run_interrupted_after_cleanup() {
    use std::io::{BufRead, Read};

    let root = TempDir::new().unwrap();
    let mut command = command(&root, "delay");
    command.args(["run", "--output", "jsonl", "wait"]);
    let mut child = command.spawn().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut stream = String::new();
    stdout.read_line(&mut stream).unwrap();
    let started: Value = serde_json::from_str(&stream).unwrap();
    assert_eq!(started["type"], "run_started");

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
    assert_eq!(run_ended(&events), ("interrupted", 143));
}

fn run(root: &TempDir, mode: &str, args: &[&str], input: Option<&str>) -> Output {
    let mut command = command(root, mode);
    command.args(args);
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        // Avoid inheriting a redirected parent stdin, which `rho run` now rejects
        // unless `--stdin` is set.
        command.stdin(Stdio::null());
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

fn command(root: &TempDir, mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
    command
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("RHO_HOME", root.path().join(".rho"))
        .env(MODE_ENV, mode)
        .env_remove(RESPONSE_ENV)
        .env_remove(PATH_ENV)
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

fn jsonl_events(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The session events in a JSONL stream's `events`, in order.
fn session_events(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|line| line["type"] == "event")
        .map(|line| &line["event"])
        .collect()
}

/// The reason and exit code of the stream's one `run_ended`, its last line.
fn run_ended(events: &[Value]) -> (&str, u64) {
    let ended: Vec<&Value> = events
        .iter()
        .filter(|line| line["type"] == "run_ended")
        .collect();
    assert_eq!(ended, [events.last().unwrap()], "{events:#?}");
    let last = events.last().unwrap();
    (
        last["reason"].as_str().unwrap(),
        last["exit_code"].as_u64().unwrap(),
    )
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
