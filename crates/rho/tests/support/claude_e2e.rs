//! Shared helpers for fake-Claude runtime PTY end-to-end scenarios.
//!
//! These keep the real user path (agent tool -> executor -> `claude -p` spawn)
//! offline: a committed stream-json fixture is replayed by a scripted `claude`
//! on PATH, while spawn argv/cwd/stdin are recorded for assertions.

#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::Value;

/// Directory of committed Claude E2E fixtures under `tests/fixtures/claude_e2e`.
pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_e2e")
}

/// Install the deterministic `claude-planner` agent under the isolated home.
pub fn install_claude_planner_agent(home: &Path) -> PathBuf {
    let agents = home.join(".rho/agents");
    fs::create_dir_all(&agents).expect("create agents dir");
    let dest = agents.join("claude-planner.md");
    fs::copy(fixture_dir().join("claude-planner.md"), &dest).expect("copy agent definition");
    dest
}

/// Paths produced by a fake `claude` binary for one E2E scenario.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct FakeClaudePaths {
    pub bin_dir: PathBuf,
    pub claude: PathBuf,
    pub(crate) record_dir: PathBuf,
    pub argv_path: PathBuf,
    pub cwd_path: PathBuf,
    pub stdin_path: PathBuf,
    pub spawn_marker: PathBuf,
    pub mode: FakeClaudeMode,
}

#[derive(Clone, Copy, Debug)]
pub enum FakeClaudeMode {
    Success,
    Error,
}

impl FakeClaudeMode {
    fn payload_name(self) -> &'static str {
        match self {
            Self::Success => "success.ndjson",
            Self::Error => "error.ndjson",
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Error => 1,
        }
    }
}

/// Build a fake `claude` that answers auth probes and records `-p` spawns.
pub fn install_fake_claude(root: &Path, mode: FakeClaudeMode) -> FakeClaudePaths {
    let bin_dir = root.join("bin");
    let record_dir = root.join("record");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    fs::create_dir_all(&record_dir).expect("create fake record dir");

    let claude = bin_dir.join("claude");
    let argv_path = record_dir.join("argv.txt");
    let cwd_path = record_dir.join("cwd.txt");
    let stdin_path = record_dir.join("stdin.txt");
    let spawn_marker = record_dir.join("spawned");
    let payload_src = fixture_dir().join(mode.payload_name());
    let payload = record_dir.join(mode.payload_name());
    fs::copy(&payload_src, &payload).expect("copy stream fixture");

    // Keep a marker file the fake refuses to network-touch; any attempt would be
    // a bug in the script (it only reads local paths).
    let offline_guard = record_dir.join("offline-only");
    fs::write(&offline_guard, b"no-network\n").expect("write offline guard");

    let script = format!(
        r#"#!/bin/sh
set -eu
# Deterministic offline Claude Code stub for Rho PTY E2E.
# Never contacts the network; only reads/writes local fixture paths.

argv_path={argv_path}
cwd_path={cwd_path}
stdin_path={stdin_path}
spawn_marker={spawn_marker}
payload={payload}
exit_code={exit_code}

# Auth / version probes used by Rho login and session preflight.
if [ "${{1-}}" = "auth" ] && [ "${{2-}}" = "status" ]; then
  printf '%s\n' '{{"loggedIn":true,"email":"fake-e2e@example.com","subscriptionType":"pro","authMethod":"claude.ai"}}'
  exit 0
fi
if [ "${{1-}}" = "auth" ] && [ "${{2-}}" = "logout" ]; then
  exit 0
fi
if [ "${{1-}}" = "--version" ] || [ "${{1-}}" = "version" ]; then
  printf '%s\n' '0.0.0-fake-e2e'
  exit 0
fi

# Production subagent path: `claude -p ...` with the prompt on stdin.
if [ "${{1-}}" = "-p" ]; then
  # Record exact argv (NUL-separated) and cwd before consuming stdin.
  : > "$argv_path"
  for arg in "$@"; do
    printf '%s\0' "$arg" >> "$argv_path"
  done
  pwd > "$cwd_path"
  touch "$spawn_marker"
  # Replay stream-json first so the parent can observe a terminal result and
  # close stdin. With --input-format stream-json Rho keeps stdin open until
  # that result arrives; reading stdin to EOF first would deadlock.
  cat "$payload"
  cat > "$stdin_path"
  exit "$exit_code"
fi

printf 'unexpected fake claude args:' >&2
printf ' %s' "$@" >&2
printf '\n' >&2
exit 99
"#,
        argv_path = shell_single_quote(&argv_path),
        cwd_path = shell_single_quote(&cwd_path),
        stdin_path = shell_single_quote(&stdin_path),
        spawn_marker = shell_single_quote(&spawn_marker),
        payload = shell_single_quote(&payload),
        exit_code = mode.exit_code(),
    );
    fs::write(&claude, script).expect("write fake claude");
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).expect("chmod fake claude");

    FakeClaudePaths {
        bin_dir,
        claude,
        record_dir,
        argv_path,
        cwd_path,
        stdin_path,
        spawn_marker,
        mode,
    }
}

fn shell_single_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// Prepend the fake bin directory so Rho resolves this `claude`, not a host one.
pub fn path_with_fake(bin_dir: &Path) -> String {
    let host = std::env::var("PATH").unwrap_or_default();
    if host.is_empty() {
        bin_dir.display().to_string()
    } else {
        format!("{}:{host}", bin_dir.display())
    }
}

/// Paths for a login-only fake `claude` used by `/login claude-code` PTY tests.
#[derive(Debug)]
#[allow(dead_code)]
pub struct FakeClaudeLogin {
    pub bin_dir: tempfile::TempDir,
    pub claude: PathBuf,
    pub marker: PathBuf,
    pub path: String,
}

/// Install a minimal `claude` that answers auth status/login/version probes.
///
/// On successful `auth login --claudeai` it touches `marker` so tests can prove
/// the external binary ran. Status reports signed-in only after that marker exists.
pub fn install_fake_claude_login() -> FakeClaudeLogin {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = tempfile::tempdir().expect("fake claude login bin dir");
    let claude = bin_dir.path().join("claude");
    let marker = bin_dir.path().join("login-ran");
    fs::write(
        &claude,
        format!(
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  if [ -f "{marker}" ]; then
    printf '%s\n' '{{"loggedIn":true,"email":"fake@example.com","subscriptionType":"pro"}}'
  else
    printf '%s\n' '{{"loggedIn":false}}'
  fi
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "login" ] && [ "$3" = "--claudeai" ]; then
  printf 'FAKE_CLAUDE_LOGIN_READY\n'
  touch "{marker}"
  exit 0
fi
if [ "$1" = "--version" ]; then
  printf '%s\n' '0.0.0-fake'
  exit 0
fi
echo "unexpected args: $*" >&2
exit 1
"#,
            marker = marker.display(),
        ),
    )
    .expect("write fake claude login");
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755))
        .expect("chmod fake claude login");
    let path = path_with_fake(bin_dir.path());
    FakeClaudeLogin {
        bin_dir,
        claude,
        marker,
        path,
    }
}

/// Wait until the fake Claude `-p` spawn has been recorded.
pub fn wait_for_spawn(paths: &FakeClaudePaths, timeout: Duration) {
    wait_until(timeout, "fake claude spawn", || paths.spawn_marker.exists());
}

/// Poll until `predicate` is true or `timeout` elapses.
pub fn wait_until(timeout: Duration, label: &str, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    loop {
        if predicate() {
            return;
        }
        if started.elapsed() >= timeout {
            panic!("timed out waiting for {label} after {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Parsed spawn record from the fake Claude binary.
#[derive(Debug)]
pub struct SpawnRecord {
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: String,
}

impl FakeClaudePaths {
    pub fn read_spawn_record(&self) -> SpawnRecord {
        let raw = fs::read(&self.argv_path).expect("read argv record");
        let args = raw
            .split(|byte| *byte == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| String::from_utf8(chunk.to_vec()).expect("argv utf-8"))
            .collect::<Vec<_>>();
        let cwd = fs::read_to_string(&self.cwd_path)
            .expect("read cwd record")
            .trim()
            .to_string();
        let stdin = fs::read_to_string(&self.stdin_path).expect("read stdin record");
        SpawnRecord {
            args,
            cwd: PathBuf::from(cwd),
            stdin,
        }
    }
}

/// Assert production spawn flags for a closed Claude-cli planner run.
pub fn assert_success_spawn(record: &SpawnRecord, workspace: &Path) {
    let args = &record.args;
    assert!(
        args.first().map(String::as_str) == Some("-p"),
        "expected -p first, got {args:?}"
    );
    assert_pair(args, "--output-format", "stream-json");
    assert_pair(args, "--input-format", "stream-json");
    assert_contains(args, "--verbose");
    assert_contains(args, "--include-partial-messages");
    assert_pair(args, "--permission-mode", "bypassPermissions");
    assert_pair(args, "--disallowedTools", "Task");
    assert_pair(args, "--setting-sources", "project");
    assert_contains(args, "--strict-mcp-config");
    assert_pair(args, "--model", "claude-opus-demo");
    assert_pair(args, "--max-turns", "10000");
    assert_pair(args, "--tools", "Read,Edit,Bash");
    assert_allowed_tools(args, &["Read", "Edit", "Bash(git *)"]);
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--system-prompt-file"),
        "missing --system-prompt-file: {args:?}"
    );
    assert_each_task_is_disallowed_tools_value(args);
    // Task must never be in --tools or --allowedTools.
    if let Some(tools) = value_after(args, "--tools") {
        assert!(
            !tools
                .split(',')
                .any(|name| name.eq_ignore_ascii_case("Task")),
            "--tools must not include Task: {tools}"
        );
    }
    if let Some(idx) = args.iter().position(|arg| arg == "--allowedTools") {
        let allowed = &args[idx + 1..];
        assert!(
            !allowed
                .iter()
                .any(|entry| entry.eq_ignore_ascii_case("Task")
                    || entry.to_ascii_lowercase().starts_with("task(")),
            "--allowedTools must not include Task: {allowed:?}"
        );
    }
    let expected_cwd = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let actual_cwd = record
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| record.cwd.clone());
    assert_eq!(
        actual_cwd, expected_cwd,
        "claude cwd should be the Rho workspace"
    );
    let expected_prompt = "Say hello in one short sentence.";
    let expected_stdin = format!(
        "{}\n",
        serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": expected_prompt,
            },
        })
    );
    assert_eq!(
        record.stdin, expected_stdin,
        "prompt must arrive on stdin as one stream-json user turn"
    );
}

/// Every `Task` argv token must be the immediate value of `--disallowedTools`.
fn assert_each_task_is_disallowed_tools_value(args: &[String]) {
    for (index, arg) in args.iter().enumerate() {
        if arg != "Task" {
            continue;
        }
        let is_disallowed_value = index
            .checked_sub(1)
            .is_some_and(|flag_idx| args[flag_idx] == "--disallowedTools");
        assert!(
            is_disallowed_value,
            "Task at argv[{index}] must be immediately after --disallowedTools: {args:?}"
        );
    }
}

fn assert_allowed_tools(args: &[String], expected: &[&str]) {
    let idx = args
        .iter()
        .position(|arg| arg == "--allowedTools")
        .unwrap_or_else(|| panic!("missing --allowedTools in {args:?}"));
    let got = &args[idx + 1..];
    // allowedTools consumes the rest of argv until another flag would start; our
    // spawn plan puts it last before optional system-prompt flags, so take the
    // expected count.
    let got = &got[..expected.len().min(got.len())];
    assert_eq!(
        got, expected,
        "--allowedTools mismatch\n  got: {got:?}\n  exp: {expected:?}\n  full: {args:?}"
    );
}

fn assert_pair(args: &[String], flag: &str, value: &str) {
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value),
        "missing {flag} {value} in {args:?}"
    );
}

fn assert_contains(args: &[String], flag: &str) {
    assert!(
        args.iter().any(|arg| arg == flag),
        "missing {flag} in {args:?}"
    );
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

/// List global and folder-session run directories that contain a result file.
pub fn list_run_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = run_dirs_directly_under(&home.join(".rho/subagents"));
    let sessions_root = home.join(".rho/sessions");
    if let Ok(workspaces) = fs::read_dir(sessions_root) {
        for workspace in workspaces.filter_map(Result::ok) {
            let Ok(sessions) = fs::read_dir(workspace.path()) else {
                continue;
            };
            for session in sessions.filter_map(Result::ok) {
                dirs.extend(run_dirs_directly_under(&session.path().join("subagents")));
            }
        }
    }
    dirs.sort();
    dirs
}

fn run_dirs_directly_under(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("result.json").is_file())
        .collect()
}

/// Wait until exactly one subagent run directory exists with a terminal result.
pub fn wait_for_single_run_dir(home: &Path, timeout: Duration) -> PathBuf {
    wait_until(timeout, "single subagent run dir", || {
        list_run_dirs(home).len() == 1
    });
    list_run_dirs(home)
        .into_iter()
        .next()
        .expect("run dir present after wait")
}

/// Wait until `result.json` reaches a terminal state.
pub fn wait_for_terminal_result(run_dir: &Path, timeout: Duration) -> Value {
    let path = run_dir.join("result.json");
    wait_until(timeout, "terminal result.json", || {
        read_json(&path)
            .ok()
            .and_then(|value| {
                value
                    .get("state")?
                    .as_str()
                    .map(|state| state != "starting" && state != "running")
            })
            .unwrap_or(false)
    });
    read_json(&path).expect("result.json")
}

pub fn read_json(path: &Path) -> Result<Value, String> {
    let body = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&body).map_err(|error| error.to_string())
}

/// Assert the success-path `result.json` contract for the live fixture.
pub fn assert_success_result(status: &Value, run_dir: &Path) {
    assert_eq!(status["state"], "ok", "status={status}");
    assert_eq!(status["agent_id"], "claude-planner");
    assert_eq!(status["provider"], "claude-code");
    assert_eq!(status["result"], "rho-claude-e2e-ok");
    assert_eq!(
        status["claude_session_id"],
        "11111111-2222-4333-8444-555555555555"
    );
    assert!(
        status
            .get("agent_fingerprint")
            .and_then(Value::as_str)
            .is_some(),
        "fingerprint missing: {status}"
    );
    assert_eq!(
        status
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .map(|cost| (cost * 1_000_000.0).round() as u64),
        Some(34_271),
        "expected fixture total_cost_usd on terminal status: {status}"
    );
    let events = run_dir.join("events.jsonl");
    assert!(
        events.is_file(),
        "events.jsonl missing at {}",
        events.display()
    );
    let events_body = fs::read_to_string(&events).expect("read events");
    // Live fixture streams the final text as partial deltas ("r" + "ho-claude-e2e-ok").
    // result.json holds the joined terminal text; events must still carry both halves.
    assert!(
        events_body.contains("assistant_text_delta"),
        "events missing assistant deltas:\n{events_body}"
    );
    assert!(
        events_body.contains("ho-claude-e2e-ok"),
        "events missing final text tail:\n{events_body}"
    );
    assert!(
        events_body.contains("\"type\":\"completed\"")
            || events_body.contains("\"type\": \"completed\""),
        "events missing completed marker:\n{events_body}"
    );
    // log.txt is opened for Claude stderr; presence proves the session adapter
    // reached spawn rather than failing closed earlier.
    assert!(
        run_dir.join("log.txt").is_file(),
        "log.txt missing beside {}",
        run_dir.display()
    );
}

/// Assert the error-path `result.json` contract.
pub fn assert_error_result(status: &Value) {
    assert_eq!(status["state"], "error", "status={status}");
    assert_eq!(status["agent_id"], "claude-planner");
    assert_eq!(status["provider"], "claude-code");
    let error = status
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !error.is_empty()
            || status
                .get("result")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty()),
        "error status should carry detail: {status}"
    );
}
