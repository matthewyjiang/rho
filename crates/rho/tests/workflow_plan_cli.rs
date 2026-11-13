//! Planning workflows through the real `rho` binary on the target it runs on.
#![cfg(unix)]

use std::process::{Command, Stdio};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

const CONFIG: &str = r#"[model]
provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"
"#;

const COMMAND_WORKFLOW: &str = r#"def build(inputs):
    return workflow(name = "command", nodes = [
        agent(
            name = "inspect",
            agent = "reviewer",
            prompt = "inspect the workspace",
            access = "mutating",
            timeout_seconds = 600,
            max_output_bytes = 100000,
        ),
        command(
            name = "build",
            argv = ["/bin/sh", "-c", "true"],
            cwd = ".",
            timeout_seconds = 10,
            max_output_bytes = 100,
        ),
    ])

WORKFLOW = define(inputs = {}, build = build)
"#;

const AGENT_WORKFLOW: &str = r#"def build(inputs):
    return workflow(name = "agent-only", nodes = [
        agent(
            name = "inspect",
            agent = "reviewer",
            prompt = "inspect the workspace",
            access = "mutating",
            timeout_seconds = 600,
            max_output_bytes = 100000,
        ),
    ])

WORKFLOW = define(inputs = {}, build = build)
"#;

/// The error a frozen command launch gives where the OS adapter cannot launch
/// from verified handles.
const UNSUPPORTED_LAUNCH: &str = "frozen workflow process execution is unavailable on this platform because the OS adapter cannot launch the executable, interpreter, and working directory from verified handles";

// Covers: `rho workflow plan` rejects a workflow with a command node before
// approval on a target without handle-based launch, with the launch's own
// error, and freezes it where commands launch; a workflow of in-process Rho
// agents plans on every target (#1736).
// Owner: workflow planning (CLI plan path on the real target).
#[test]
fn planning_rejects_command_nodes_only_where_they_cannot_launch() {
    let handle_launch = cfg!(any(target_os = "linux", target_os = "android"));
    let cases = [
        ("command.star", COMMAND_WORKFLOW, handle_launch),
        ("agent.star", AGENT_WORKFLOW, true),
    ];
    for (file, source, plans) in cases {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("config.toml"), CONFIG).unwrap();
        std::fs::write(root.path().join(file), source).unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_rho"))
            .current_dir(root.path())
            .env("HOME", root.path())
            .env("RHO_HOME", root.path().join(".rho"))
            // Keep credential probes off the developer's session bus as well as HOME.
            .env("XDG_RUNTIME_DIR", root.path())
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_PANE_ID")
            .stdin(Stdio::null())
            .arg("--config")
            .arg(root.path().join("config.toml"))
            .args(["workflow", "plan", file])
            .output()
            .unwrap();

        let stderr = String::from_utf8_lossy(&output.stderr);
        if plans {
            assert!(output.status.success(), "{file}: {stderr}");
        } else {
            assert_eq!(
                (output.status.success(), stderr.as_ref()),
                (false, format!("Error: {UNSUPPORTED_LAUNCH}\n").as_str()),
                "{file}"
            );
        }
    }
}
