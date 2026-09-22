//! A run saved by an earlier release is listed in the workflows hub but is
//! read-only. Opening it must report the error and keep the TUI running.

use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result};
use serde_json::json;

use crate::{env::IsolatedHome, harness::WaitTimeout, keys::Key, scenario::Step};

pub(super) const WORKFLOW_HUB_LEGACY_RUN_ID: &str = "workflow_hub_legacy_run";

const RUN_ID: &str = "1e9ac700-0000-4000-8000-000000000001";
const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");

/// Writes a completed run in the version 1 single-graph store format.
pub(super) fn setup_workflow_hub_legacy_run(home: &IsolatedHome) -> Result<()> {
    let workspace = fs::canonicalize(&home.workspace).context("canonicalize workspace")?;
    let workflows = home.home.join(".rho/workflows");
    let run = workflows.join("runs").join(RUN_ID);
    for directory in [&workflows, &workflows.join("runs"), &run] {
        create_private_directory(directory)?;
    }
    write_private_json(
        &run.join("manifest.json"),
        &json!({
            "schema_version": 1,
            "run_id": RUN_ID,
            "created_at_unix_nanos": 1,
            "plan_id": "1e9ac700-0000-4000-8000-000000000002",
            "graph_digest": "sha256:legacy",
            "workspace_identity": workspace.to_string_lossy(),
            "consent": {"graph_digest": "sha256:legacy", "confirmed": true},
            "name": "legacy",
            "step_count": 1,
        }),
    )?;
    write_private_json(
        &run.join("state.json"),
        &json!({
            "schema_version": 2,
            "last_event_sequence": 0,
            "state": {
                "revision": 1,
                "lifecycle": "completed",
                "outcome": "success",
                "cancellation_requested": false,
                "nodes": {"greet": {"state": "terminal", "outcome": "success"}},
                "command_exits": {},
                "outputs": {},
                "completions": {},
            },
        }),
    )
}

fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    fs::write(path, serde_json::to_vec(value)?)
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(super) const WORKFLOW_HUB_LEGACY_RUN_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("select_legacy_run"),
    Step::SubmitText("/workflow"),
    Step::WaitText {
        text: "WORKFLOWS",
        timeout: STARTUP,
    },
    Step::Key(Key::Down),
    Step::WaitText {
        text: "Run id 1e9ac700",
        timeout: SETTLE,
    },
    Step::Phase("open_legacy_run"),
    Step::Key(Key::Enter),
    // The hub stays open over the transcript; close it to read the error row.
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "could not load run",
        timeout: SETTLE,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Phase("tui_still_running"),
    Step::SubmitText("after legacy run"),
    Step::WaitText {
        text: "fixture response: after legacy run",
        timeout: STREAM,
    },
    Step::ExitCommand,
];
