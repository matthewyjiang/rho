//! Covers: real workflow tools must not strand chat behind an extra run form,
//! lose the structured receipt, or drop live progress after the turn ends.
//! Also covers: starting a source must not accumulate launch-only saved plans.
//! Owner: interactive workflow lifecycle; existing CLI scenarios use a matrix
//! driver and never exercise model-facing tools or source-start persistence.

use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};

use super::{assert_helpers::wait_for_turn_completion_after, DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::{Key, MouseButton},
    scenario::{Scenario, Step},
    PtyHarness,
};

const GATE: &str = ".rho-fixture-workflow-gate";
const LIVE_PROGRESS: &str = "running · 1/3 tasks · hold";

pub(super) const WORKFLOW_BACKGROUND_SCENARIO: Scenario = Scenario::new(
    "workflow_background",
    "Run real workflow tools without a second form, keep live progress, and avoid source-start plan clutter",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(workflow_background),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup_workflow);

fn setup_workflow(home: &IsolatedHome) -> Result<()> {
    let gate = CString::new(home.workspace.join(GATE).as_os_str().as_bytes())?;
    // A FIFO read blocks in the real command runner until the PTY writes it.
    // No Python, fixture runtime, polling shell, or timing-based release.
    let created = unsafe { libc::mkfifo(gate.as_ptr(), libc::S_IRUSR | libc::S_IWUSR) };
    if created != 0 {
        return Err(std::io::Error::last_os_error()).context("create workflow release FIFO");
    }
    let directory = home.workspace.join(".rho/workflows");
    fs::create_dir_all(&directory)?;
    // The held command's failure bound covers every preceding bounded screen
    // wait. Output capacities are the exact fixed stdout bytes of each node.
    let timeout_seconds = STARTUP.duration.as_secs() + 5 * STREAM.duration.as_secs();
    fs::write(
        directory.join("pty-background.star"),
        format!(
            r#"def build(inputs):
    prepare = command(
        name = "prepare",
        argv = ["/bin/sh", "-c", "printf 'ready\\n'"],
        cwd = ".",
        timeout_seconds = {timeout_seconds},
        max_output_bytes = 6,
    )
    hold = command(
        name = "hold",
        argv = ["/bin/sh", "-c", "read release < {GATE} && test \"$release\" = release && printf 'released\\n'"],
        cwd = ".",
        needs = ["prepare"],
        timeout_seconds = {timeout_seconds},
        max_output_bytes = 9,
    )
    finish = command(
        name = "finish",
        argv = ["/bin/sh", "-c", "printf 'finished\\n'"],
        cwd = ".",
        needs = ["hold"],
        timeout_seconds = {timeout_seconds},
        max_output_bytes = 9,
    )
    return workflow(name = "pty-background", nodes = [prepare, hold, finish])

WORKFLOW = define(inputs = {{}}, build = build)
"#
        ),
    )
    .context("seed real background workflow source")
}

fn workflow_background(harness: &mut PtyHarness) -> Result<()> {
    let store = harness
        .working_directory()
        .and_then(Path::parent)
        .context("matrix workspace has no isolated home parent")?
        .join("home/.rho/workflows");
    harness.set_phase("model_plan_and_run_without_host_questionnaire");
    harness.submit_text("fixture workflow background")?;
    // No Enter/choice follows submit. A redundant run questionnaire makes this
    // wait fail before any gate release, not pass via accidental approval.
    harness.wait_for_text("workflow fixture dispatched", STREAM)?;
    wait_for_turn_completion_after(harness, "workflow fixture dispatched")?;
    harness.wait_for_text("workflow.plan", STREAM)?;
    harness.wait_for_text("workflow.run", STREAM)?;
    let rows = harness.screen().rows_text();
    ensure!(
        rows.iter().any(|row| row.contains("workflow.plan")
            && row.contains("pty-background")
            && row.contains("planned"))
            && rows.iter().any(|row| row.contains("workflow.run")),
        "workflow tools must render dedicated plan/run receipts:\n{}",
        harness.screen().debug_dump()
    );
    harness.set_phase("live_workflow_survives_turn_end");
    assert_live_rail(harness)?;
    harness.wait_for_text("1 workflow running", STREAM)?;
    harness.resize(DEFAULT_SIZE.rows, 40)?;
    harness.wait_for_text("running · 1/3", STREAM)?;
    harness.resize(DEFAULT_SIZE.rows, DEFAULT_SIZE.cols)?;
    assert_live_rail(harness)?;

    harness.set_phase("busy_rail_click_is_not_replayed_after_turn");
    harness.submit_text("fixture gated reply")?;
    harness.wait_for_text("reply waiting for release", STREAM)?;
    click_live_workflow(harness)?;
    fs::write(
        harness
            .working_directory()
            .context("workflow workspace")?
            .join(".rho-fixture-release-reply"),
        b"release",
    )?;
    harness.wait_for_text("reply completed after release", STREAM)?;
    wait_for_turn_completion_after(harness, "reply completed after release")?;

    harness.set_phase("workflow_click_preserves_questionnaire");
    harness.submit_text("fixture questionnaire")?;
    harness.wait_for_text("Choose one color", STREAM)?;
    click_live_workflow(harness)?;
    harness.inject_key(&Key::Down)?;
    harness.inject_key(&Key::Enter)?;
    let answered = "questionnaire response observed exactly 1 time";
    harness.wait_for_text(answered, STREAM)?;
    wait_for_turn_completion_after(harness, answered)?;

    harness.set_phase("rail_click_opens_watch_without_stopping_run");
    click_live_workflow(harness)?;
    harness.wait_for_text("Graph", STREAM)?;
    harness.wait_for_text("q leave", STREAM)?;
    harness.inject_key(&Key::Char('q'))?;
    assert_live_rail(harness)?;
    let original_runs = record_ids(&store.join("runs"))?;
    ensure!(
        original_runs.len() == 1,
        "expected one tool-started run: {original_runs:?}"
    );
    let original_plans = record_ids(&store.join("plans"))?;
    ensure!(
        original_plans.len() == 1,
        "expected one explicitly frozen plan: {original_plans:?}"
    );
    release_gate(harness)?;
    wait_for_completion(harness, &original_runs[0])?;

    harness.set_phase("hub_source_start_does_not_save_launch_plan");
    harness.submit_text("/workflow")?;
    harness.wait_for_text("Workflows", STREAM)?;
    harness.wait_for_text("Saved plans · 1", STREAM)?;
    let saved_rows = harness.screen().rows_text();
    ensure!(
        saved_rows
            .iter()
            .filter(|row| row.contains("Saved plans ·"))
            .count()
            == 1,
        "hub must have one saved-plan browser row:\n{}",
        harness.screen().debug_dump()
    );
    // The only source is the first hub row; source Start launches directly.
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("Starting 'pty-background'", STREAM)?;
    assert_live_rail(harness)?;
    let runs = record_ids(&store.join("runs"))?;
    ensure!(runs.len() == 2, "expected tool and source runs: {runs:?}");
    let source_run = runs
        .iter()
        .find(|id| !original_runs.contains(id))
        .context("source Start did not create a distinct run")?;
    release_gate(harness)?;
    wait_for_completion(harness, source_run)?;
    let plans = record_ids(&store.join("plans"))?;
    ensure!(
        plans == original_plans && record_ids(&store.join("runs"))? == runs,
        "source Start must retain the explicitly frozen plan without adding a launch plan; plans={plans:?} runs={runs:?}"
    );
    Ok(())
}

fn click_live_workflow(harness: &mut PtyHarness) -> Result<()> {
    assert_live_rail(harness)?;
    let row = harness
        .screen()
        .rows_text()
        .iter()
        .position(|row| row.contains("workflow pty-background") && row.contains(LIVE_PROGRESS))
        .context("live workflow rail row")? as u16
        + 1;
    harness.mouse(MouseButton::Left, 5, row, true)?;
    harness.mouse(MouseButton::Left, 5, row, false)
}

fn assert_live_rail(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text(LIVE_PROGRESS, STREAM)?;
    ensure!(
        harness
            .screen()
            .rows_text()
            .iter()
            .any(|row| { row.contains("workflow pty-background") && row.contains(LIVE_PROGRESS) }),
        "workflow name, lifecycle, progress and active task must share the live rail row:\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

fn release_gate(harness: &mut PtyHarness) -> Result<()> {
    let gate = harness
        .working_directory()
        .context("workflow working directory")?
        .join(GATE);
    let deadline = Instant::now() + STREAM.duration;
    loop {
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&gate)
        {
            Ok(mut writer) => {
                return writer
                    .write_all(b"release\n")
                    .context("release workflow command")
            }
            Err(error)
                if error.raw_os_error() == Some(libc::ENXIO)
                    || error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(error).context("open workflow release FIFO"),
        }
        ensure!(
            harness.is_running() && Instant::now() < deadline,
            "workflow command never opened its release FIFO:\n{}",
            harness.screen().debug_dump()
        );
        // Poll PTY output while waiting on the explicit reader-ready condition.
        harness.poll(Duration::from_millis(25));
    }
}

fn wait_for_completion(harness: &mut PtyHarness, run_id: &str) -> Result<()> {
    harness.wait_for_text(
        &format!("workflow {run_id} (pty-background) finished - success"),
        STREAM,
    )?;
    // Source starts also append a plain transcript notice with the workflow
    // name. Only the tree row should disappear when its result is delivered.
    harness.wait_for_text_gone("└ workflow pty-background", STREAM)?;
    let response = format!("workflow fixture completion incorporated {run_id}");
    harness.wait_for_text(&response, STREAM)?;
    wait_for_turn_completion_after(harness, &response)
}

fn record_ids(directory: &Path) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            ids.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("non-UTF-8 workflow record id"))?,
            );
        }
    }
    ids.sort();
    Ok(ids)
}
