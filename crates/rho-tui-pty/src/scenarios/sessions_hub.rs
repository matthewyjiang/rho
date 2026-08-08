use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result};
use serde_json::json;

use crate::{env::IsolatedHome, harness::WaitTimeout, keys::Key, scenario::Step};

const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

const QUIET: Step = Step::WaitQuiet {
    quiet_for: Duration::from_millis(150),
    timeout: SETTLE,
};

/// Seed one valid foreign transcript and one transcript misplaced under the
/// launch workspace. The scenario proves only the valid owner is discoverable.
pub(super) fn setup_sessions_hub(home: &IsolatedHome) -> Result<()> {
    let foreign_cwd = home.path().join("foreign-workspace");
    fs::create_dir_all(&foreign_cwd).context("create foreign session workspace")?;
    write_seed_session(
        home,
        &foreign_cwd,
        &foreign_cwd,
        "11111111-0000-4000-8000-000000000001",
        "foreign workspace target",
    )?;

    let launch_cwd = std::env::current_dir().context("read launch workspace")?;
    write_seed_session(
        home,
        &launch_cwd,
        &foreign_cwd,
        "22222222-0000-4000-8000-000000000002",
        "misplaced foreign workspace target",
    )?;
    Ok(())
}

fn write_seed_session(
    home: &IsolatedHome,
    physical_cwd: &Path,
    recorded_cwd: &Path,
    id: &str,
    text: &str,
) -> Result<()> {
    let session_dir = home
        .home
        .join(".rho/sessions")
        .join(workspace_key(physical_cwd));
    fs::create_dir_all(&session_dir).context("create seeded session directory")?;
    let header = json!({
        "type": "session",
        "version": 3,
        "id": id,
        "timestamp": "1",
        "cwd": recorded_cwd,
    });
    let message = json!({
        "type": "message",
        "timestamp": "2",
        "message": {"User": [{"Text": text}]},
    });
    fs::write(
        session_dir.join(format!("1_{id}.jsonl")),
        format!("{header}\n{message}\n"),
    )
    .context("write seeded session transcript")
}

fn workspace_key(cwd: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let display = cwd.to_string_lossy();
    let encoded = display
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    let hash = display.as_bytes().iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    });
    format!("{encoded}-{hash:016x}")
}

/// Reject direct and hub-based resume of a foreign session, then browse and
/// resume a local session. Exercise directory-wide delete by cancelling,
/// confirming, and verifying the current session survives.
pub(super) const SESSIONS_HUB_STEPS: &[Step] = &[
    Step::Phase("create_saved_session"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("sessions hub target"),
    Step::WaitText {
        text: "fixture response: sessions hub target",
        timeout: STREAM,
    },
    Step::Phase("start_fresh_session"),
    Step::Key(Key::Ctrl('r')),
    Step::WaitText {
        text: "conversation reset",
        timeout: SETTLE,
    },
    Step::Phase("reject_misplaced_foreign_transcript"),
    Step::SubmitText("/resume 22222222"),
    Step::WaitText {
        text: "no session found matching '22222222'",
        timeout: SETTLE,
    },
    Step::Phase("reject_direct_cross_directory_resume"),
    Step::SubmitText("/resume 11111111"),
    Step::WaitText {
        text: "could not resume session: start Rho in",
        timeout: SETTLE,
    },
    Step::Phase("open_sessions_hub"),
    Step::SubmitText("/sessions"),
    Step::WaitText {
        text: "All sessions",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "foreign workspace target",
        timeout: SETTLE,
    },
    Step::Phase("reject_cross_directory_resume"),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Esc back",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Start Rho in this directory to resume",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "start Rho in that directory to resume this session",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "All sessions",
        timeout: SETTLE,
    },
    Step::Key(Key::Up),
    Step::Key(Key::Up),
    // Rows label sessions by stored title, so confirm the saved session by
    // selecting it and reading its last user message in the detail pane.
    Step::Key(Key::Down),
    Step::WaitText {
        text: "last: sessions hub target",
        timeout: SETTLE,
    },
    Step::Key(Key::Up),
    Step::Phase("browse_directory"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Esc back",
        timeout: SETTLE,
    },
    Step::Phase("resume_from_directory"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "resumed session",
        timeout: SETTLE,
    },
    Step::Phase("reopen_hub"),
    Step::SubmitText("/sessions"),
    Step::WaitText {
        text: "All sessions",
        timeout: SETTLE,
    },
    Step::Phase("cancel_directory_delete"),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "Delete all sessions in",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "All sessions",
        timeout: SETTLE,
    },
    QUIET,
    Step::Phase("confirm_directory_delete_keeps_current"),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "Delete all sessions in",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "kept the current session",
        timeout: SETTLE,
    },
    Step::Phase("current_session_delete_is_refused"),
    Step::Key(Key::Down),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "cannot delete the current session",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    QUIET,
    Step::ExitCommand,
];
