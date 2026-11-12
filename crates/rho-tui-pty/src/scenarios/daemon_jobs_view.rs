//! `/jobs` lists the daemon's jobs across projects, answers a job's
//! permission prompt without attaching to its session, and starts, cancels,
//! messages, filters, and attaches to sessions (#1668).

use anyhow::{ensure, Result};

use super::{daemon_resume::write_daemon_config_in_mode, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner, Step},
};

const ENV: &[(&str, &str)] = &[("OPENAI_API_KEY", "sk-test-matrix")];

// Covers: from `/jobs`, `n` starts a session in a workspace with a first
// prompt and it lists as running; a filter keeps the rows that match and
// says when none does, and Esc clears it; `c` twice cancels the selected
// session's turn, and once only asks; `m` sends it a message that starts its next turn; Enter
// attaches, and this TUI then follows that session's turn to its end.
// Owner: interactive TUI as a daemon client
pub(super) const ACTIONS_SCENARIO: Scenario = Scenario::new(
    "daemon_jobs_view_actions",
    "Start, cancel, message, filter, and attach to a session from /jobs",
    DEFAULT_SIZE,
    ACTIONS_STEPS,
    /*smoke*/ false,
)
.with_setup(super::daemon_resume::write_daemon_config)
.with_env(ENV);

const ACTIONS_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open"),
    Step::SubmitText("/jobs"),
    // This TUI's own session, loaded and between turns.
    Step::WaitText {
        text: "▸ idle",
        timeout: SETTLE,
    },
    Step::Phase("new_session"),
    Step::Key(Key::Char('n')),
    Step::WaitText {
        text: "Workspace:",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Prompt:",
        timeout: SETTLE,
    },
    Step::TypeText("fixture gated reply"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "  running",
        timeout: STREAM,
    },
    // Rows are picked by filter: two sessions opened in the same second
    // list in either order.
    Step::Phase("cancel"),
    Step::Key(Key::Char('/')),
    Step::TypeText("running"),
    Step::WaitText {
        text: "▸ running",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    // The first `c` asks, the second cancels.
    Step::Key(Key::Char('c')),
    Step::WaitText {
        text: "c again cancels",
        timeout: SETTLE,
    },
    Step::AssertText("▸ running"),
    Step::Key(Key::Char('c')),
    // Cancelled, so it no longer matches.
    Step::WaitText {
        text: "No jobs match the filter",
        timeout: STREAM,
    },
    Step::Phase("message"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Filter:",
        timeout: SETTLE,
    },
    Step::Key(Key::Char('/')),
    Step::TypeText("ended"),
    Step::WaitText {
        text: "▸ ended",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::Key(Key::Char('m')),
    Step::WaitText {
        text: "Message:",
        timeout: SETTLE,
    },
    Step::TypeText("fixture gated reply"),
    Step::Key(Key::Enter),
    // Running again, so it no longer matches.
    Step::WaitText {
        text: "No jobs match the filter",
        timeout: STREAM,
    },
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Filter:",
        timeout: SETTLE,
    },
    Step::Key(Key::Char('/')),
    Step::TypeText("running"),
    Step::WaitText {
        text: "▸ running",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::Phase("attach"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "reply waiting for release",
        timeout: STREAM,
    },
    Step::Custom(|harness| {
        super::fixture_release::release_fixture(harness, ".rho-fixture-release-reply")
    }),
    Step::WaitText {
        text: "reply completed after release",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

// Covers: from `/jobs`, Tab while starting a session opts it into a git
// worktree of its own (#1673): the session starts, and its row shows the
// branch the daemon made for it, under this project.
// Owner: interactive TUI as a daemon client
pub(super) const WORKTREE_SCENARIO: Scenario = Scenario::new(
    "daemon_jobs_view_worktree",
    "Start a session in its own git worktree from /jobs and see its branch",
    DEFAULT_SIZE,
    WORKTREE_STEPS,
    /*smoke*/ false,
)
.with_setup(setup_repository)
.with_env(ENV);

/// The daemon config, in a workspace that is a git repository with one
/// commit.
fn setup_repository(home: &IsolatedHome) -> Result<()> {
    super::daemon_resume::write_daemon_config(home)?;
    let repo = &home.workspace;
    std::fs::write(repo.join("notes.md"), "base\n")?;
    super::diff_viewer::git(repo, &["init", "-q", "-b", "main"])?;
    super::diff_viewer::git(repo, &["add", "notes.md"])?;
    super::diff_viewer::git(repo, &["commit", "-q", "-m", "init"])?;
    Ok(())
}

const WORKTREE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open"),
    Step::SubmitText("/jobs"),
    Step::WaitText {
        text: "▸ idle",
        timeout: SETTLE,
    },
    Step::Phase("new_session_in_a_worktree"),
    Step::Key(Key::Char('n')),
    Step::WaitText {
        text: "Worktree: off",
        timeout: SETTLE,
    },
    Step::Key(Key::Tab),
    Step::WaitText {
        text: "Worktree: new git worktree and branch",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Prompt:",
        timeout: SETTLE,
    },
    Step::TypeText("hello"),
    Step::Key(Key::Enter),
    // The row's first fact; the branch is named for the repository's
    // directory, `workspace`.
    Step::WaitText {
        text: "· rho/workspace-",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_jobs_view",
    "List jobs running in two projects and answer one's permission prompt from /jobs",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

/// The second project's directory name, which its group in the view shows.
const OTHER_PROJECT: &str = "other-project";

// Covers: with a turn running in this TUI's project and a detached run in
// another project waiting on a permission prompt, `/jobs` lists both under
// their projects; Tab selects the job that needs input and shows what it
// asks, and `y` answers the prompt from the view, so the other project's
// turn goes on while this TUI still follows its own session.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config_in_mode(&home, "supervised")?;
    let other = home.path().join(OTHER_PROJECT);
    std::fs::create_dir_all(&other)?;
    // Its `write` call asks for approval, with no client attached.
    let detached = home
        .command(&runner.binary)
        .current_dir(&other)
        .args(["run", "--detach", "fixture tool"])
        .env("RHO_TUI_TEST_MODE", "matrix")
        // The daemon this starts reads its fixtures' release markers here,
        // for every session it hosts.
        .env("RHO_TUI_FIXTURE_WORKSPACE", &home.workspace)
        .env("OPENAI_API_KEY", "sk-test-matrix")
        .output()?;
    ensure!(
        detached.status.success(),
        "rho run --detach failed: {}\ndaemon log:\n{}",
        String::from_utf8_lossy(&detached.stderr),
        home.daemon_log()
    );
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let workspace = home
        .workspace
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result = (|| -> Result<()> {
        harness.set_phase("launch");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.set_phase("run_a_turn_here");
        harness.submit_text("fixture gated reply")?;
        harness.wait_for_text("esc abort", STREAM)?;

        harness.set_phase("list_both_projects");
        harness.submit_text("/jobs")?;
        // The view opens on this TUI's own job, so nothing is asked yet.
        harness.wait_for_screen("both projects' jobs", SETTLE, |screen| {
            let text = screen.contents();
            text.contains("Jobs")
                && text.contains(&workspace)
                && text.contains(OTHER_PROJECT)
                && text.contains("▸ running")
                && text.contains("needs input")
                && !text.contains("write wants to write")
        })?;

        harness.set_phase("answer_the_other_projects_prompt");
        harness.inject_key(&Key::Tab)?;
        harness.wait_for_text("write wants to write", SETTLE)?;
        harness.inject_key(&Key::Char('y'))?;
        harness.wait_for_text_gone("needs input", STREAM)?;

        harness.set_phase("finish_the_turn_here");
        harness.inject_key(&Key::Esc)?;
        super::fixture_release::release_fixture(&mut harness, ".rho-fixture-release-reply")?;
        harness.wait_for_text("reply completed after release", STREAM)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the TUI exited with code {code}");
        Ok(())
    })();

    if result.is_err() && harness.is_running() {
        let _ = harness.kill();
    }
    Ok(ScenarioOutcome {
        id: SCENARIO.id.into(),
        passed: result.is_ok(),
        message: result
            .err()
            .map(|error| format!("{error:#}\ndaemon log:\n{}", home.daemon_log()))
            .unwrap_or_default(),
        timing: harness.timing().clone(),
        artifact_dir: runner.artifact_root.clone(),
    })
}
