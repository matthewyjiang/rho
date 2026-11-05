//! Shared helpers for the fake ACP runtime PTY end-to-end scenarios.
//!
//! The real delegated path (agent tool -> executor -> ACP agent spawn -> ACP
//! driver) runs offline: a shim named like the vendor binary sits on PATH,
//! records how Rho spawned it, then execs the debug-only
//! `rho __acp-fixture-agent`, which plays a committed script and journals
//! every request and reply it saw.

#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::Value;

/// One vendor shim: binary name, fixture directory under `tests/fixtures`,
/// and the environment variable whose spawn-time value is recorded, if any.
pub struct FakeAcpAgent {
    pub program: &'static str,
    pub fixtures: &'static str,
    pub recorded_env: Option<&'static str>,
    /// Required argument of the ACP spawn, when the vendor binary also serves
    /// other commands (Cursor's and agy's model and status probes) that must
    /// fail fast instead of overwriting the spawn record.
    pub acp_subcommand: Option<&'static str>,
    /// Shell run first, for non-ACP commands the shim answers itself (such as
    /// Claude's `--cli auth status`). It must `exit` for those.
    pub prelude: &'static str,
}

impl FakeAcpAgent {
    pub fn fixture_dir(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(self.fixtures)
    }

    /// Install `<agent_file>` from the fixtures under the isolated home.
    pub fn install_agent(&self, home: &Path, agent_file: &str) {
        let agents = home.join(".rho/agents");
        fs::create_dir_all(&agents).expect("create agents dir");
        fs::copy(self.fixture_dir().join(agent_file), agents.join(agent_file))
            .expect("copy agent definition");
    }

    /// Build the shim that records the spawn and execs `rho` on
    /// `success.json`.
    pub fn install(&self, root: &Path, rho: &Path) -> FakeAcpPaths {
        self.install_script(root, rho, "success.json")
    }

    /// Build the shim that records the spawn and execs `rho` on `script`
    /// from the fixture directory.
    pub fn install_script(&self, root: &Path, rho: &Path, script: &str) -> FakeAcpPaths {
        let bin_dir = root.join("bin");
        let record_dir = root.join("record");
        fs::create_dir_all(&bin_dir).expect("create fake bin dir");
        fs::create_dir_all(&record_dir).expect("create fake record dir");
        let paths = FakeAcpPaths {
            program: bin_dir.join(self.program),
            bin_dir,
            argv_path: record_dir.join("argv"),
            cwd_path: record_dir.join("cwd"),
            env_path: record_dir.join("env"),
            journal_path: record_dir.join("journal.jsonl"),
        };
        let guard = match self.acp_subcommand {
            Some(subcommand) => format!(
                r#"found=
for arg in "$@"; do case "$arg" in {subcommand}) found=1 ;; esac; done
if [ -z "$found" ]; then
  printf 'unexpected fake ACP agent args:' >&2
  printf ' %s' "$@" >&2
  printf '\n' >&2
  exit 99
fi
"#
            ),
            None => String::new(),
        };
        let shim = format!(
            r#"#!/bin/sh
set -eu
{prelude}
{guard}: > {argv}
for arg in "$@"; do printf '%s\0' "$arg" >> {argv}; done
pwd > {cwd}
{record_env} > {env_record}
exec {rho} __acp-fixture-agent --script {script} --journal {journal}
"#,
            argv = quote(&paths.argv_path),
            cwd = quote(&paths.cwd_path),
            record_env = match self.recorded_env {
                Some(env) => format!("printf '%s' \"${{{env}-}}\""),
                None => ":".to_owned(),
            },
            env_record = quote(&paths.env_path),
            rho = quote(rho),
            script = quote(&self.fixture_dir().join(script)),
            journal = quote(&paths.journal_path),
            prelude = self.prelude,
        );
        fs::write(&paths.program, shim).expect("write fake ACP agent");
        fs::set_permissions(&paths.program, fs::Permissions::from_mode(0o755))
            .expect("chmod fake ACP agent");
        paths
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// Paths written by the shim and the scripted agent.
pub struct FakeAcpPaths {
    pub bin_dir: PathBuf,
    pub program: PathBuf,
    argv_path: PathBuf,
    cwd_path: PathBuf,
    env_path: PathBuf,
    journal_path: PathBuf,
}

/// How Rho spawned the shim.
#[derive(Debug)]
pub struct AcpSpawnRecord {
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The recorded environment variable; empty when unset or not recorded.
    pub env: String,
}

impl FakeAcpPaths {
    /// Read after the run is terminal: the shim writes these before exec.
    pub fn spawn_record(&self) -> AcpSpawnRecord {
        let args = fs::read(&self.argv_path)
            .expect("read argv record")
            .split(|byte| *byte == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| String::from_utf8(chunk.to_vec()).expect("argv utf-8"))
            .collect();
        let read = |path: &Path| fs::read_to_string(path).expect("read spawn record");
        AcpSpawnRecord {
            args,
            cwd: PathBuf::from(read(&self.cwd_path).trim()),
            env: read(&self.env_path),
        }
    }

    /// Every journal entry, in order: `{"request": ...}` or `{"reply": ...}`.
    /// Replies are journaled before the scripted `end_turn`, so a terminal
    /// `result.json` implies a complete journal.
    pub fn journal(&self) -> Vec<Value> {
        fs::read_to_string(&self.journal_path)
            .expect("read scripted agent journal")
            .lines()
            .map(|line| serde_json::from_str(line).expect("journal line is JSON"))
            .collect()
    }
}

/// Prepend the fake bin directory so Rho resolves the fake, not a host binary.
pub fn path_with_fake(bin_dir: &Path) -> String {
    let host = std::env::var("PATH").unwrap_or_default();
    if host.is_empty() {
        bin_dir.display().to_string()
    } else {
        format!("{}:{host}", bin_dir.display())
    }
}

/// Resolve `program` on a PATH string the same way a shell would (first hit).
pub fn which_on_path(program: &str, path_var: &str) -> Option<PathBuf> {
    for dir in path_var.split(':').filter(|dir| !dir.is_empty()) {
        let candidate = PathBuf::from(dir).join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
