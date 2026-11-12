use super::*;

// Covers: a send whose conversation holds provider-native context the active
// model cannot replay opens a confirm-send modal; Esc returns the prompt and
// "Send anyway" starts the turn. With multiple follow-ups, rejection or a
// failed confirmed turn must park the remainder rather than arm idle delivery.
// "Send for this session" stops the modal for the unchanged omitted blocks.
// "Compact, then continue" compacts on the source model and continues on the
// runtime model, after which a send no longer asks. A compaction that leaves
// the conversation as it was still sends the prompt "Compact, then send"
// held, and "Compact, then continue" then stays on the source model.
// Owner: interactive TUI
pub(super) const SEND_CONFIRM_HANDOFF_ID: &str = "send_confirm_handoff";
// Covers: the same handoff and confirmations when the daemon holds the
// conversation, which the TUI then reads from the daemon's report; and a
// "Compact, then send" the daemon cannot start, while the session's MCP
// servers connect, returns the prompt to the composer with nothing queued.
// Owner: interactive TUI as a daemon client
#[cfg(unix)]
pub(super) const DAEMON_SEND_CONFIRM_HANDOFF_ID: &str = "daemon_send_confirm_handoff";

pub(super) const SCENARIO: crate::scenario::Scenario = crate::scenario::Scenario::new(
    SEND_CONFIRM_HANDOFF_ID,
    "Confirm a send whose conversation holds unreplayable native context",
    SIZE,
    &[],
    /*smoke*/ false,
);
#[cfg(unix)]
pub(super) const DAEMON_SCENARIO: crate::scenario::Scenario = crate::scenario::Scenario::new(
    DAEMON_SEND_CONFIRM_HANDOFF_ID,
    "Confirm a daemon session's send whose conversation holds unreplayable native context",
    SIZE,
    &[],
    /*smoke*/ false,
);

const SIZE: crate::pty::PtySize = crate::pty::PtySize {
    rows: 24,
    cols: 100,
};

/// Where the scenario's sessions run.
#[derive(Clone, Copy)]
pub(crate) enum SessionHost {
    InProcess,
    #[cfg(unix)]
    Daemon,
}

impl SessionHost {
    fn id(self) -> &'static str {
        match self {
            Self::InProcess => SEND_CONFIRM_HANDOFF_ID,
            #[cfg(unix)]
            Self::Daemon => DAEMON_SEND_CONFIRM_HANDOFF_ID,
        }
    }

    fn launch(self, plan: crate::env::RhoLaunchPlan) -> crate::env::RhoLaunchPlan {
        match self {
            Self::InProcess => plan.with_env("RHO_TUI_DAEMON", "0"),
            #[cfg(unix)]
            Self::Daemon => plan.with_env("RHO_TUI_DAEMON", "1"),
        }
    }
}

/// The host `name` runs this scenario on, when it names it.
pub(crate) fn send_confirm_scenario(name: &str) -> Option<SessionHost> {
    match name {
        SEND_CONFIRM_HANDOFF_ID => Some(SessionHost::InProcess),
        #[cfg(unix)]
        DAEMON_SEND_CONFIRM_HANDOFF_ID => Some(SessionHost::Daemon),
        _ => None,
    }
}

const SEED_PROMPT: &str = "hello there";
const SEND_PROMPT: &str = "confirmed prompt";
const SESSION_PROMPT: &str = "acknowledged prompt";
const UNGATED_PROMPT: &str = "ungated prompt";
const REFUSED_COMPACT_PROMPT: &str = "refused compact prompt";
const COMPACTED_PROMPT: &str = "compacted prompt";
const AFTER_UNCHANGED_COMPACT_PROMPT: &str = "prompt after an unchanged compact";

#[derive(Clone, Copy)]
enum QueuedStop {
    Rejected,
    Failed,
}

fn assert_queued_stop(
    harness: &mut crate::harness::PtyHarness,
    stop: QueuedStop,
) -> anyhow::Result<()> {
    let case = match stop {
        QueuedStop::Rejected => "reject",
        QueuedStop::Failed => "fail",
    };
    let prompt = format!("fixture queued confirmation {case}");
    let parked = format!("parked {case} follow-up");
    harness.set_phase(format!("queue_before_{case}"));
    harness.submit_text(&prompt)?;
    harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
    harness.inject_key(&Key::Char('1'))?;
    harness.wait_for_text(&format!("queue checkpoint: {prompt}"), STREAM)?;
    harness.type_text("fixture stream failure")?;
    harness.inject_key(&Key::AltEnter)?;
    harness.wait_for_text("1 follow-up", SETTLE)?;
    harness.type_text(&parked)?;
    harness.inject_key(&Key::CtrlEnter)?;
    harness.wait_for_text("2 follow-ups", SETTLE)?;
    super::release_fixture(harness, ".rho-fixture-release-send-confirm")?;
    harness.wait_for_text("Send to openai/gpt-5.5?", STREAM)?;

    harness.set_phase(format!("confirmed_queue_{case}"));
    match stop {
        QueuedStop::Rejected => {
            // Esc cancels regardless of which options the modal has drawn yet.
            harness.inject_key(&Key::Esc)?;
            harness.wait_for_text("send cancelled", SETTLE)?;
            harness.assert_screen_contains("fixture stream failure")?;
        }
        QueuedStop::Failed => {
            harness.inject_key(&Key::Char('1'))?;
            harness.wait_for_text("deterministic forced stream termination", STREAM)?;
        }
    }

    // A unique local-command receipt proves idle input ran after the boundary.
    // The broken idle arm instead consumes the parked prompt into another modal.
    harness.inject_key(&Key::Ctrl('c'))?;
    harness.submit_text(&format!("!!printf '%s%s\\n' queue-stopped- {case}"))?;
    harness.wait_for_text(&format!("queue-stopped-{case}"), STREAM)?;
    harness.assert_screen_contains("1 follow-up")?;
    harness.assert_screen_contains(&parked)?;
    anyhow::ensure!(
        !harness.screen().contains_text("Send to openai/gpt-5.5?"),
        "idle continuation consumed a parked follow-up after {case}"
    );

    // Retract the parked item rather than send it, keeping the next case isolated.
    harness.inject_key(&Key::AltUp)?;
    harness.wait_for_text("editing queued follow-up", SETTLE)?;
    harness.inject_key(&Key::Ctrl('c'))?;
    Ok(())
}

/// A later resume of the scenario's session.
#[derive(Clone, Copy)]
enum ResumePhase {
    /// "Compact, then send" while the session's MCP servers connect again
    /// after the daemon restarted in place: the daemon session cannot
    /// compact, so the prompt goes back to the composer and nothing is
    /// queued.
    #[cfg(unix)]
    SendRefused,
    /// "Compact, then send" whose compaction leaves the conversation as it
    /// was still sends the prompt, as in process.
    SendUnchanged,
    /// "Compact, then continue" whose compaction leaves the conversation as
    /// it was keeps the session on the source model, and says so, as in
    /// process.
    ContinueUnchanged,
    /// "Compact, then continue" on the loaded-session handoff: the session
    /// compacts on the native blocks' model, then sends on the runtime model
    /// without asking, since the blocks were summarized away.
    ContinueCompacted,
}

impl ResumePhase {
    /// Resumes `session_id` on `host`, runs this phase, and quits. Config
    /// the phase writes lasts for this resume alone.
    fn run(
        self,
        runner: &crate::scenario::ScenarioRunner,
        home: &crate::env::IsolatedHome,
        host: SessionHost,
        (session_id, session_path): (&str, &std::path::Path),
    ) -> anyhow::Result<()> {
        use crate::{artifacts::ArtifactWriter, env::RhoLaunchPlan, harness::PtyHarness};

        // A daemon that hosted the last run holds the session and config it
        // loaded.
        home.stop_daemon();
        let config = std::fs::read_to_string(&home.config_path)?;
        let result = (|| {
            // A sandbox has no model cache, so the provider's default is the
            // Anthropic model a daemon session may switch to.
            let session = std::fs::read_to_string(session_path)?;
            std::fs::write(
                session_path,
                session.replace("\"claude-fable-5\"", "\"claude-sonnet-4-5\""),
            )?;
            let mut plan = RhoLaunchPlan::matrix(&runner.binary, home, SIZE)
                .with_env("OPENAI_API_KEY", "sk-test-matrix")
                .with_arg("--resume")
                .with_arg(session_id);
            if matches!(self, Self::ContinueUnchanged | Self::ContinueCompacted) {
                // The native blocks' model, so the handoff can compact on it.
                plan = plan.with_env("ANTHROPIC_API_KEY", "sk-test-matrix");
            }
            let plan = host.launch(plan);
            let mut harness = PtyHarness::spawn_named(&plan, host.id())?;
            harness.enable_timing(runner.record_timing);
            if let Some(root) = &runner.artifact_root {
                harness.set_artifact_writer(ArtifactWriter::new(root));
            }
            let result = self.drive(runner, home, host, &mut harness).and_then(|()| {
                let code = harness.quit_with_exit_command()?;
                anyhow::ensure!(code == 0, "session exited with code {code}");
                Ok(())
            });
            if harness.is_running() {
                let _ = harness.kill();
            }
            result
        })();
        home.stop_daemon();
        std::fs::write(&home.config_path, config)?;
        result
    }

    fn drive(
        self,
        runner: &crate::scenario::ScenarioRunner,
        home: &crate::env::IsolatedHome,
        host: SessionHost,
        harness: &mut crate::harness::PtyHarness,
    ) -> anyhow::Result<()> {
        match self {
            #[cfg(unix)]
            Self::SendRefused => {
                harness.set_phase("compact_send_refused");
                harness.wait_for_text("How should Rho continue", STARTUP)?;
                harness.inject_key(&Key::Enter)?;
                harness.wait_for_text("Type a message", SETTLE)?;
                harness.submit_text(REFUSED_COMPACT_PROMPT)?;
                harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
                harness.wait_for_text("Compact, then send", SETTLE)?;
                // A server that never connects holds the restarted daemon's
                // session in loading for the rest of the phase.
                let config = std::fs::read_to_string(&home.config_path)?;
                std::fs::write(
                    &home.config_path,
                    format!(
                        r#"{config}

[mcp.servers.slow-stdio]
transport = "stdio"
command = "sleep"
args = ["120"]
"#
                    ),
                )?;
                let restart = home
                    .command(&runner.binary)
                    .args(["daemon", "restart"])
                    .output()?;
                anyhow::ensure!(
                    restart.status.success(),
                    "rho daemon restart failed: {}",
                    String::from_utf8_lossy(&restart.stderr)
                );
                harness.wait_for_text("reconnected to rho daemon", STREAM)?;
                harness.inject_key(&Key::Char('2'))?;
                harness.wait_for_text("could not compact before send", SETTLE)?;
                harness.assert_screen_contains(REFUSED_COMPACT_PROMPT)?;
                anyhow::ensure!(
                    !harness.screen().contains_text("follow-up"),
                    "a compaction that never started queued the prompt"
                );
                harness.inject_key(&Key::Ctrl('c'))?;
            }
            Self::SendUnchanged => {
                harness.set_phase("compact_send_unchanged");
                harness.wait_for_text("How should Rho continue", STARTUP)?;
                harness.inject_key(&Key::Enter)?;
                harness.wait_for_text("Type a message", SETTLE)?;
                harness.submit_text(AFTER_UNCHANGED_COMPACT_PROMPT)?;
                harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
                harness.wait_for_text("Compact, then send", SETTLE)?;
                harness.inject_key(&Key::Char('2'))?;
                harness.wait_for_text("did not reduce context", STREAM)?;
                harness.wait_for_text(
                    &format!("fixture response: {AFTER_UNCHANGED_COMPACT_PROMPT}"),
                    STREAM,
                )?;
            }
            Self::ContinueUnchanged => {
                harness.set_phase("compact_continue_unchanged");
                harness.wait_for_text("How should Rho continue", STARTUP)?;
                harness.wait_for_text("Compact, then continue", SETTLE)?;
                harness.inject_key(&Key::Char('2'))?;
                harness.wait_for_text("did not reduce context", STREAM)?;
                harness.wait_for_text(
                    "kept the session model because context was not compacted",
                    STREAM,
                )?;
                // In process the source is the session's saved model, which
                // the session already runs on.
                if !matches!(host, SessionHost::InProcess) {
                    harness.wait_for_text("Anthropic ·", SETTLE)?;
                }
            }
            Self::ContinueCompacted => {
                harness.set_phase("compact_then_continue");
                harness.wait_for_text("How should Rho continue", STARTUP)?;
                harness.wait_for_text("Compact, then continue", SETTLE)?;
                harness.inject_key(&Key::Char('2'))?;
                harness.wait_for_text("context compacted", STREAM)?;
                harness.submit_text(COMPACTED_PROMPT)?;
                harness.wait_for_text(&format!("fixture response: {COMPACTED_PROMPT}"), STREAM)?;
                // Back on the runtime model once the compaction ended.
                harness.assert_screen_contains("OpenAI · gpt-5.5")?;
            }
        }
        Ok(())
    }
}

/// Starts a session on `host` that sends `prompt`, then rewrites its saved
/// answer to hold Anthropic-native context its OpenAI model cannot replay.
/// Returns the session's id and file.
fn seed_session(
    runner: &crate::scenario::ScenarioRunner,
    home: &crate::env::IsolatedHome,
    host: SessionHost,
    prompt: &str,
) -> anyhow::Result<(String, std::path::PathBuf)> {
    use crate::{artifacts::ArtifactWriter, env::RhoLaunchPlan, harness::PtyHarness};

    let plan = host.launch(
        RhoLaunchPlan::matrix(&runner.binary, home, SIZE)
            .with_env("OPENAI_API_KEY", "sk-test-matrix"),
    );
    let mut seed = PtyHarness::spawn_named(&plan, "send_confirm_seed")?;
    seed.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        seed.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> anyhow::Result<()> {
        seed.wait_for_text("gpt-5.5", STARTUP)?;
        seed.submit_text(prompt)?;
        seed.wait_for_text(&format!("fixture response: {prompt}"), STREAM)?;
        let code = seed.quit_with_exit_command()?;
        anyhow::ensure!(code == 0, "seed session exited with code {code}");
        Ok(())
    })();
    if seed.is_running() {
        let _ = seed.kill();
    }
    result?;
    let (session_id, session_path) = config::find_latest_session(home)?;
    // A daemon that hosted the seed still holds the session it saved.
    home.stop_daemon();
    // Keep stored provider openai so resume stays on gpt-5.5 while assistant
    // history carries anthropic-native blocks the runtime cannot replay.
    config::inject_non_replayable_provider_context(
        &session_path,
        config::StoredProviderRewrite::Keep,
    )?;
    Ok((session_id, session_path))
}

pub(super) fn run_send_confirm_handoff(
    runner: &crate::scenario::ScenarioRunner,
    host: SessionHost,
) -> anyhow::Result<crate::scenario::ScenarioOutcome> {
    use crate::{
        artifacts::ArtifactWriter,
        env::{IsolatedHome, RhoLaunchPlan},
        harness::PtyHarness,
        scenario::ScenarioOutcome,
    };

    let home = IsolatedHome::new()?;
    let (session_id, session_path) = match seed_session(runner, &home, host, SEED_PROMPT) {
        Ok(session) => session,
        Err(error) => {
            return Ok(ScenarioOutcome {
                id: host.id().into(),
                passed: false,
                message: format!("seed phase failed: {error:#}"),
                timing: Default::default(),
                artifact_dir: runner.artifact_root.clone(),
            })
        }
    };

    let resume_plan = host.launch(
        RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
            .with_env("OPENAI_API_KEY", "sk-test-matrix")
            .with_arg("--resume")
            .with_arg(&session_id),
    );
    let mut harness = PtyHarness::spawn_named(&resume_plan, host.id())?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> anyhow::Result<()> {
        // The loaded-session handoff opens first; continue with the runtime
        // model so the native blocks stay in history and gate the next send.
        harness.set_phase("loaded_session_handoff_opens");
        harness.wait_for_text("How should Rho continue", STARTUP)?;
        harness.inject_key(&Key::Enter)?;
        // The resume status is a transient toast; the durable signal that the
        // handoff resolved is the composer accepting input again.
        harness.wait_for_text("Type a message", SETTLE)?;

        harness.set_phase("send_opens_confirm_modal");
        harness.submit_text(SEND_PROMPT)?;
        harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
        harness.assert_screen_contains("Send anyway")?;
        harness.assert_screen_contains("Don't send")?;

        harness.set_phase("esc_returns_prompt");
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("send cancelled", SETTLE)?;
        harness.assert_screen_contains(SEND_PROMPT)?;

        harness.set_phase("send_anyway_starts_turn");
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
        harness.inject_key(&Key::Char('1'))?;
        harness.wait_for_text(&format!("fixture response: {SEND_PROMPT}"), STREAM)?;
        for stop in [QueuedStop::Rejected, QueuedStop::Failed] {
            assert_queued_stop(&mut harness, stop)?;
        }

        // Runs last: the acknowledgement suppresses every later modal for
        // these unchanged blocks, which the cases above depend on.
        harness.set_phase("send_for_session_acknowledges");
        harness.submit_text(SESSION_PROMPT)?;
        harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
        // Session consent is the last option and needs its text on screen.
        harness.wait_for_text("Send for this session", SETTLE)?;
        harness.inject_key(&Key::Char('4'))?;
        harness.wait_for_text(&format!("fixture response: {SESSION_PROMPT}"), STREAM)?;

        harness.set_phase("unchanged_omissions_skip_confirm");
        harness.submit_text(UNGATED_PROMPT)?;
        // The modal would block the turn, so the response is the proof.
        harness.wait_for_text(&format!("fixture response: {UNGATED_PROMPT}"), STREAM)?;
        let code = harness.quit_with_exit_command()?;
        if code != 0 {
            anyhow::bail!("session exited with code {code}");
        }
        #[cfg(unix)]
        if matches!(host, SessionHost::Daemon) {
            ResumePhase::SendRefused.run(runner, &home, host, (&session_id, &session_path))?;
        }
        // The compaction summarizes the native blocks away.
        ResumePhase::ContinueCompacted.run(runner, &home, host, (&session_id, &session_path))?;
        // Each on a fresh session of one turn, too short for a compaction to
        // reduce.
        for phase in [ResumePhase::SendUnchanged, ResumePhase::ContinueUnchanged] {
            let (session_id, session_path) = seed_session(runner, &home, host, SEED_PROMPT)?;
            phase.run(runner, &home, host, (&session_id, &session_path))?;
        }
        Ok(())
    })();

    Ok(match result {
        Ok(()) => ScenarioOutcome {
            id: host.id().into(),
            passed: true,
            message: "ok".into(),
            timing: harness.timing().clone(),
            artifact_dir: None,
        },
        Err(error) => {
            if harness.is_running() {
                let _ = harness.kill();
            }
            ScenarioOutcome {
                id: host.id().into(),
                passed: false,
                message: format!("{error:#}"),
                timing: harness.timing().clone(),
                artifact_dir: runner.artifact_root.clone(),
            }
        }
    })
}
