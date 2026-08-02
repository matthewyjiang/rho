use std::time::Duration;

use anyhow::Result;

use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    pty::PtySize,
    scenario::{ScenarioOutcome, ScenarioRunner},
};

pub(super) const WORKFLOW_RUN_ID: &str = "workflow_run_interactive";
pub(super) const WORKFLOW_CANCEL_RESUME_ID: &str = "workflow_cancel_resume";

const PLAN_ID: &str = "00000000-0000-4000-8000-000000000674";
const RUN_ID: &str = "00000000-0000-4000-8000-000000000675";
const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};
const STARTUP: WaitTimeout = WaitTimeout::secs(20, "workflow TUI startup");
const UPDATE: WaitTimeout = WaitTimeout::secs(10, "workflow update");

pub(super) fn is_workflow_scenario(name: &str) -> bool {
    matches!(name, WORKFLOW_RUN_ID | WORKFLOW_CANCEL_RESUME_ID)
}

pub(super) fn run(runner: &ScenarioRunner, name: &str) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    let result = match name {
        WORKFLOW_RUN_ID => run_to_completion(runner, &home),
        WORKFLOW_CANCEL_RESUME_ID => cancel_then_resume(runner, &home),
        _ => unreachable!("workflow scenario dispatch checked the id"),
    };

    Ok(match result {
        Ok(timing) => ScenarioOutcome {
            id: name.into(),
            passed: true,
            message: "ok".into(),
            timing,
            artifact_dir: None,
        },
        Err(error) => ScenarioOutcome {
            id: name.into(),
            passed: false,
            message: format!("{error:#}"),
            timing: Default::default(),
            artifact_dir: runner.artifact_root.clone(),
        },
    })
}

fn run_to_completion(
    runner: &ScenarioRunner,
    home: &IsolatedHome,
) -> Result<crate::timing::TimingSummary> {
    let plan = workflow_plan(runner, home, &["workflow", "run", PLAN_ID]);
    let mut harness = spawn(runner, &plan, WORKFLOW_RUN_ID)?;
    let result = (|| -> Result<()> {
        // Owner plan gate: footer shows start; header is ready.
        harness.wait_for_text("enter start", STARTUP)?;
        harness.wait_for_text("matrix workflow", STARTUP)?;
        harness.assert_raw_contains(b"\x1b[?1049h")?;
        harness.inject_key(&Key::Enter)?;
        // Parallel agents start after confirm. The running-node labels
        // (`running · try N`) are transient in-flight state that the matrix
        // fixture advances on a timer, so PTY asserts only stable rendered
        // nodes here; the attempt label itself is covered deterministically in
        // `dag_tests` (running_node_label_reports_attempt).
        harness.wait_for_text("Inspect workspace", UPDATE)?;
        harness.wait_for_text("Run checks", UPDATE)?;
        harness.inject_key(&Key::Down)?;
        // Progress event on the test node, then apply becomes live.
        harness.wait_for_text("checks complete", UPDATE)?;
        harness.resize(20, 72)?;
        harness.wait_for_text("Apply result", UPDATE)?;
        harness.resize(SIZE.rows, SIZE.cols)?;
        harness.wait_for_text("finished · success", UPDATE)?;
        harness.wait_for_text("workflow completed", UPDATE)?;
        // Leave is allowed once the run is durable; footer may truncate with notices.
        harness.wait_for_quiet(Duration::from_millis(150), UPDATE)?;
        harness.inject_key(&Key::Char('q'))?;
        let code = harness.wait_for_exit(UPDATE)?;
        if code != 0 {
            anyhow::bail!("workflow process exited with code {code}");
        }
        assert_terminal_restored(&harness)
    })();
    finish(harness, result)
}

fn cancel_then_resume(
    runner: &ScenarioRunner,
    home: &IsolatedHome,
) -> Result<crate::timing::TimingSummary> {
    let run_plan = workflow_plan(runner, home, &["workflow", "run", PLAN_ID]);
    let mut first = spawn(runner, &run_plan, "workflow_cancel")?;
    let first_result = (|| -> Result<()> {
        first.wait_for_text("enter start", STARTUP)?;
        first.inject_key(&Key::Enter)?;
        // Confirm puts the run in a live, cancellable state. As in
        // run_to_completion, avoid racing the transient `running · try N`
        // labels; assert a stable node and then cancel.
        first.wait_for_text("Inspect workspace", UPDATE)?;
        first.inject_key(&Key::Char('c'))?;
        first.wait_for_text("finished · cancelled", UPDATE)?;
        first.wait_for_text("rho workflow resume", UPDATE)?;
        first.wait_for_quiet(Duration::from_millis(150), UPDATE)?;
        first.inject_key(&Key::Char('q'))?;
        let code = first.wait_for_exit(UPDATE)?;
        if code != 0 {
            anyhow::bail!("cancelled workflow process exited with code {code}");
        }
        assert_terminal_restored(&first)
    })();
    let first_timing = finish(first, first_result)?;

    let resume_plan = workflow_plan(runner, home, &["workflow", "resume", RUN_ID]);
    let mut resumed = spawn(runner, &resume_plan, "workflow_resume")?;
    let resume_result = (|| -> Result<()> {
        resumed.wait_for_text("enter continue", STARTUP)?;
        // Completed inspect is preserved on the resume matrix path.
        resumed.wait_for_text("Inspect workspace", UPDATE)?;
        resumed.inject_key(&Key::Enter)?;
        // The run is live after confirm. We assert the stable lifecycle label
        // rather than the transient node-level `running · try 2` (which the
        // matrix fixture advances on a timer); resume does not rerun the
        // completed node, and that durable behavior is covered at the runtime
        // layer and by the deterministic attempt-record tests.
        resumed.wait_for_text("running", UPDATE)?;
        resumed.wait_for_text("finished · success", UPDATE)?;
        resumed.wait_for_quiet(Duration::from_millis(150), UPDATE)?;
        resumed.inject_key(&Key::Char('q'))?;
        let code = resumed.wait_for_exit(UPDATE)?;
        if code != 0 {
            anyhow::bail!("resumed workflow process exited with code {code}");
        }
        assert_terminal_restored(&resumed)
    })();
    let resumed_timing = finish(resumed, resume_result)?;

    let mut timing = first_timing;
    timing.samples.extend(resumed_timing.samples);
    Ok(timing)
}

fn workflow_plan(runner: &ScenarioRunner, home: &IsolatedHome, args: &[&str]) -> RhoLaunchPlan {
    args.iter().fold(
        RhoLaunchPlan::matrix(&runner.binary, home, SIZE),
        |plan, arg| plan.with_arg(*arg),
    )
}

fn spawn(runner: &ScenarioRunner, plan: &RhoLaunchPlan, name: &str) -> Result<PtyHarness> {
    let mut harness = PtyHarness::spawn_named(plan, name)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    Ok(harness)
}

fn finish(mut harness: PtyHarness, result: Result<()>) -> Result<crate::timing::TimingSummary> {
    match result {
        Ok(()) => Ok(harness.timing().clone()),
        Err(error) => {
            if harness.is_running() {
                let _ = harness.kill();
            }
            Err(error)
        }
    }
}

fn assert_terminal_restored(harness: &PtyHarness) -> Result<()> {
    let raw = harness.raw_output();
    if raw.windows(8).any(|window| window == b"\x1b[?1049l")
        || String::from_utf8_lossy(raw).contains("?1049l")
    {
        Ok(())
    } else {
        anyhow::bail!("workflow TUI did not leave the alternate screen")
    }
}
