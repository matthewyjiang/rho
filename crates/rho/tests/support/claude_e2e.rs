//! Fake `claude-agent-acp` for the Claude runtime and login PTY scenarios.
//!
//! Delegated runs and the advisor spawn the adapter with no arguments and
//! speak ACP; the shim execs the scripted ACP agent (see `acp_e2e`). The
//! adapter also runs its bundled Claude CLI with `--cli`; the shim answers
//! those probes itself so they never overwrite the spawn record.

#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use super::acp_e2e::{path_with_fake, FakeAcpAgent};

/// `--cli` probes a signed-in adapter answers during a run (`/info`,
/// `/doctor`, pickers); anything else under `--cli` fails loudly.
const SIGNED_IN_CLI_PRELUDE: &str = r#"if [ "${1-}" = "--cli" ]; then
  if [ "${2-}" = "auth" ] && [ "${3-}" = "status" ]; then
    printf '%s\n' '{"loggedIn":true,"email":"fake@example.com","subscriptionType":"max"}'
    exit 0
  fi
  if [ "${2-}" = "--version" ]; then
    printf '%s\n' '0.0.0-fake (Claude Code)'
    exit 0
  fi
  printf 'unexpected fake claude-agent-acp args:' >&2
  printf ' %s' "$@" >&2
  printf '\n' >&2
  exit 99
fi"#;

pub const FAKE_CLAUDE: FakeAcpAgent = FakeAcpAgent {
    program: "claude-agent-acp",
    fixtures: "claude_acp",
    recorded_env: None,
    acp_subcommand: None,
    prelude: SIGNED_IN_CLI_PRELUDE,
};

/// Paths for a login-only fake adapter used by `/login claude-code` PTY tests.
#[derive(Debug)]
#[allow(dead_code)]
pub struct FakeClaudeLogin {
    pub bin_dir: tempfile::TempDir,
    pub adapter: PathBuf,
    pub marker: PathBuf,
    pub path: String,
}

/// Install a minimal `claude-agent-acp` that answers `--cli` auth status,
/// login, and version probes.
///
/// On a successful `--cli auth login --claudeai` it touches `marker` so tests
/// can prove the external binary ran. Status reports signed in only after
/// that marker exists.
pub fn install_fake_claude_login() -> FakeClaudeLogin {
    let bin_dir = tempfile::tempdir().expect("fake claude login bin dir");
    let adapter = bin_dir.path().join("claude-agent-acp");
    let marker = bin_dir.path().join("login-ran");
    fs::write(
        &adapter,
        format!(
            r#"#!/bin/sh
if [ "$1" != "--cli" ]; then
  echo "unexpected args: $*" >&2
  exit 1
fi
shift
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
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755))
        .expect("chmod fake claude login");
    let path = path_with_fake(bin_dir.path());
    FakeClaudeLogin {
        bin_dir,
        adapter,
        marker,
        path,
    }
}
