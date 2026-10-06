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
};

use serde_json::Value;

/// One vendor shim: binary name, fixture directory under `tests/fixtures`,
/// and the environment variable whose spawn-time value is recorded.
pub struct FakeAcpAgent {
    pub program: &'static str,
    pub fixtures: &'static str,
    pub recorded_env: &'static str,
    /// Required last argument of the ACP spawn, when the vendor binary also
    /// serves other commands (Cursor's model and status probes) that must
    /// fail fast instead of overwriting the spawn record.
    pub acp_subcommand: Option<&'static str>,
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
                r#"last=
for arg in "$@"; do last=$arg; done
if [ "$last" != "{subcommand}" ]; then
  printf 'unexpected fake ACP agent args:' >&2
  printf ' %s' "$@" >&2
  printf '\n' >&2
  exit 99
fi
"#
            ),
            None => String::new(),
        };
        let script = format!(
            r#"#!/bin/sh
set -eu
{guard}: > {argv}
for arg in "$@"; do printf '%s\0' "$arg" >> {argv}; done
pwd > {cwd}
printf '%s' "${{{env}-}}" > {env_record}
exec {rho} __acp-fixture-agent --script {script} --journal {journal}
"#,
            argv = quote(&paths.argv_path),
            cwd = quote(&paths.cwd_path),
            env = self.recorded_env,
            env_record = quote(&paths.env_path),
            rho = quote(rho),
            script = quote(&self.fixture_dir().join("success.json")),
            journal = quote(&paths.journal_path),
        );
        fs::write(&paths.program, script).expect("write fake ACP agent");
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
    /// The recorded environment variable; empty when unset.
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
