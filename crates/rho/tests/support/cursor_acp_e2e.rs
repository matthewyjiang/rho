//! Shared helpers for the fake-Cursor ACP runtime PTY end-to-end scenario.
//!
//! The real delegated path (agent tool -> executor -> `cursor-agent acp` spawn
//! -> ACP driver) runs offline: a `cursor-agent` shim on PATH records how Rho
//! spawned it, then execs the debug-only `rho __acp-fixture-agent`, which plays
//! a committed script and journals every request and reply it saw.

#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde_json::Value;

/// Committed fixtures under `tests/fixtures/cursor_acp`.
pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cursor_acp")
}

/// Install the `cursor-worker` agent definition under the isolated home.
pub fn install_cursor_worker_agent(home: &Path) {
    let agents = home.join(".rho/agents");
    fs::create_dir_all(&agents).expect("create agents dir");
    fs::copy(
        fixture_dir().join("cursor-worker.md"),
        agents.join("cursor-worker.md"),
    )
    .expect("copy agent definition");
}

/// Paths written by the `cursor-agent` shim and the scripted agent.
pub struct FakeCursorPaths {
    pub bin_dir: PathBuf,
    pub cursor_agent: PathBuf,
    argv_path: PathBuf,
    cwd_path: PathBuf,
    config_dir_path: PathBuf,
    journal_path: PathBuf,
}

/// How Rho spawned the shim.
#[derive(Debug)]
pub struct CursorSpawnRecord {
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub config_dir: PathBuf,
}

/// Build a `cursor-agent` shim that records the ACP spawn and execs `rho`.
pub fn install_fake_cursor_agent(root: &Path, rho: &Path) -> FakeCursorPaths {
    let bin_dir = root.join("bin");
    let record_dir = root.join("record");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    fs::create_dir_all(&record_dir).expect("create fake record dir");
    let paths = FakeCursorPaths {
        cursor_agent: bin_dir.join("cursor-agent"),
        bin_dir,
        argv_path: record_dir.join("argv"),
        cwd_path: record_dir.join("cwd"),
        config_dir_path: record_dir.join("config_dir"),
        journal_path: record_dir.join("journal.jsonl"),
    };
    let script = format!(
        r#"#!/bin/sh
set -eu
# Offline cursor-agent stub: only the ACP spawn is expected.
last=
for arg in "$@"; do last=$arg; done
if [ "$last" != "acp" ]; then
  printf 'unexpected fake cursor-agent args:' >&2
  printf ' %s' "$@" >&2
  printf '\n' >&2
  exit 99
fi
: > {argv}
for arg in "$@"; do printf '%s\0' "$arg" >> {argv}; done
pwd > {cwd}
printf '%s' "${{CURSOR_CONFIG_DIR-}}" > {config_dir}
exec {rho} __acp-fixture-agent --script {script} --journal {journal}
"#,
        argv = quote(&paths.argv_path),
        cwd = quote(&paths.cwd_path),
        config_dir = quote(&paths.config_dir_path),
        rho = quote(rho),
        script = quote(&fixture_dir().join("success.json")),
        journal = quote(&paths.journal_path),
    );
    fs::write(&paths.cursor_agent, script).expect("write fake cursor-agent");
    fs::set_permissions(&paths.cursor_agent, fs::Permissions::from_mode(0o755))
        .expect("chmod fake cursor-agent");
    paths
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

impl FakeCursorPaths {
    /// Read after the run is terminal: the shim writes these before exec.
    pub fn spawn_record(&self) -> CursorSpawnRecord {
        let args = fs::read(&self.argv_path)
            .expect("read argv record")
            .split(|byte| *byte == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| String::from_utf8(chunk.to_vec()).expect("argv utf-8"))
            .collect();
        let read = |path: &Path| fs::read_to_string(path).expect("read spawn record");
        CursorSpawnRecord {
            args,
            cwd: PathBuf::from(read(&self.cwd_path).trim()),
            config_dir: PathBuf::from(read(&self.config_dir_path)),
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
