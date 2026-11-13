//! Plans and runs saved before Rho 2.15 (version 1 manifests) stay listed in
//! the workflows hub, but this release cannot open them. Enter explains that
//! in the transcript and the TUI keeps running.

use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result};
use serde_json::json;

use crate::{env::IsolatedHome, harness::WaitTimeout, keys::Key, scenario::Step};

pub(super) const WORKFLOW_HUB_UNSUPPORTED_ID: &str = "workflow_hub_unsupported_records";

const RUN_ID: &str = "1e9ac700-0000-4000-8000-000000000001";
const PLAN_ID: &str = "1e9ac700-0000-4000-8000-000000000002";
const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");

/// Writes a plan and a run manifest in the version 1 single-graph store format.
/// Inventory reads only the manifest of such records.
pub(super) fn setup_workflow_hub_unsupported_records(home: &IsolatedHome) -> Result<()> {
    let workspace = fs::canonicalize(&home.workspace).context("canonicalize workspace")?;
    let workflows = home.home.join(".rho/workflows");
    let run = workflows.join("runs").join(RUN_ID);
    let plan = workflows.join("plans").join(PLAN_ID);
    for directory in [
        &workflows,
        &workflows.join("runs"),
        &run,
        &workflows.join("plans"),
        &plan,
    ] {
        create_private_directory(directory)?;
    }
    write_private_json(
        &plan.join("manifest.json"),
        &json!({
            "schema_version": 1, "plan_id": PLAN_ID,
            "graph_digest": "sha256:old", "source_digests": {},
            "workspace_identity": workspace.to_string_lossy(),
            "name": "old plan", "step_count": 1,
        }),
    )?;
    write_private_json(
        &run.join("manifest.json"),
        &json!({
            "schema_version": 1,
            "run_id": RUN_ID,
            "created_at_unix_nanos": 1,
            "plan_id": PLAN_ID,
            "graph_digest": "sha256:old",
            "workspace_identity": workspace.to_string_lossy(),
            "consent": {"graph_digest": "sha256:old", "confirmed": true},
            "name": "old run",
            "step_count": 1,
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

pub(super) const WORKFLOW_HUB_UNSUPPORTED_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("select_unsupported_run"),
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
    Step::Phase("open_unsupported_run"),
    Step::WaitText {
        text: "Saved by Rho before 2.15",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "could not open workflow run 1e9ac700",
        timeout: SETTLE,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Phase("open_unsupported_plan"),
    Step::SubmitText("/workflow"),
    Step::WaitText {
        text: "WORKFLOWS",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Back to workflows",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::WaitText {
        text: "Plan id 1e9ac700",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "could not open workflow plan 1e9ac700",
        timeout: SETTLE,
    },
    Step::Phase("tui_still_running"),
    Step::SubmitText("after unsupported records"),
    Step::WaitText {
        text: "fixture response: after unsupported records",
        timeout: STREAM,
    },
    Step::ExitCommand,
];
