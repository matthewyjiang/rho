//! Supervise one ACP process and funnel every outcome through one terminal write.

use super::{
    driver::{self, DriverOutcome},
    policy::AcpAgentPolicy,
    transport::child_transport,
    turn::TurnClassification,
    AcpSpawnPlan,
};
use crate::{
    agent::PromptPolicy,
    cli_runtime::{
        agent_event::{render::EventRenderer, AgentEvent},
        parent_messages::ParentMessageInbox,
        read_log_tail,
        status_sink::StatusSink,
        stream_effect::TerminalResult,
        BeforeSpawn, CliExecutable, CliSessionOverrides, OwnedChild,
    },
    run_artifacts::RunArtifactIdentity,
    session_environment::Ambient,
    subagent::RunStatus,
};
use rho_tools::cancellation::RunCancellation;
use std::{path::PathBuf, process::Stdio};
use tokio::{
    process::{ChildStdin, ChildStdout},
    sync::watch,
};

/// Executor-facing inputs. ACP session ids are protocol metadata, not resume ids.
pub(crate) struct AcpSessionRequest {
    pub(crate) identity: RunArtifactIdentity,
    pub(crate) prompt: String,
    pub(crate) system_prompt: PromptPolicy,
    pub(crate) output_file: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    pub(crate) started_status: Option<RunStatus>,
    pub(crate) parent_messages: Option<ParentMessageInbox>,
    pub(crate) overrides: CliSessionOverrides,
    /// The environment the agent runs in and is found on: its session's when
    /// the run launched (#1659).
    pub(crate) environment: Ambient,
}

pub(super) fn open_sink<P: AcpAgentPolicy>(
    request: &mut AcpSessionRequest,
    policy: &P,
) -> anyhow::Result<StatusSink> {
    match request.started_status.take() {
        Some(status) => StatusSink::continue_from(
            request.output_file.clone(),
            status,
            &request.prompt,
            request.status_tx.take(),
            request.overrides.live_title.clone(),
            policy.rate_limit_recorder(),
            policy.label(),
        ),
        None => StatusSink::new(
            request.output_file.clone(),
            &request.identity,
            &request.prompt,
            request.status_tx.take(),
            policy.rate_limit_recorder(),
            policy.label(),
        ),
    }
}

fn prepare<P: AcpAgentPolicy>(
    request: &AcpSessionRequest,
    policy: &mut P,
    renderer: &mut EventRenderer,
    sink: &mut StatusSink,
) -> Result<String, String> {
    let run_dir = request
        .output_file
        .parent()
        .ok_or("acp: result path has no run directory")?;
    for notice in policy.prepare(run_dir)? {
        driver::render(renderer, sink, AgentEvent::Notice(notice));
    }
    match &request.system_prompt {
        PromptPolicy::Extend(system) if !system.trim().is_empty() => {
            Ok(format!("{system}\n\n{}", request.prompt))
        }
        PromptPolicy::Extend(_) => Ok(request.prompt.clone()),
        PromptPolicy::Replace(_) => Err(
            "acp: system prompt replacement is unsupported by the session/prompt protocol".into(),
        ),
    }
}

pub(super) async fn run_child<P: AcpAgentPolicy>(
    request: &mut AcpSessionRequest,
    mut policy: P,
    renderer: &mut EventRenderer,
    sink: &mut StatusSink,
) -> Result<DriverOutcome, String> {
    if request.cancellation.is_cancelled() {
        return Ok(stopped());
    }
    let prompt = prepare(request, &mut policy, renderer, sink)?;
    let executable = match request.overrides.executable.take() {
        Some(executable) => executable,
        None => policy.resolve_executable(&request.environment)?,
    };
    let plan = policy.spawn_plan(request.overrides.frozen_argv.take())?;
    let log_path = policy.log_path(&request.output_file);
    let program = policy.label().program;
    let log = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .map_err(|source| format!("{program}: could not open log file: {source}"))?
        .into_std()
        .await;
    let (mut child, stdin, stdout) = spawn_agent(
        program,
        &executable,
        plan,
        log.into(),
        request.overrides.before_spawn.as_ref(),
        &request.environment,
    )?;
    // Move policy into the connection after setup; callers keep no mutable
    // policy while the agent is running.
    let driver = driver::run(
        child_transport(stdin, stdout, program),
        policy,
        driver::DriverContext {
            prompt: &prompt,
            cwd: &request.cwd,
            cancellation: &request.cancellation,
            inbox: &mut request.parent_messages,
            renderer,
            sink,
        },
    );
    tokio::pin!(driver);
    // Observe leader exit alongside the protocol: a descendant can inherit
    // stdout, so EOF alone may never come. OwnedChild::wait kills the rest of
    // the group, which closes the pipe; the driver then drains what was already
    // sent (a buffered stop reason still wins) and reports the closed transport.
    let (outcome, exit) = tokio::select! {
        outcome = &mut driver => (outcome, None),
        status = child.wait() => (driver.await, Some(status)),
    };
    if exit.is_none() {
        // Cursor never exits on stdin EOF after a turn (10/10 spike runs).
        // Always terminate, even on success; OwnedChild gives 200 ms then
        // kills the tree.
        child.terminate().await;
    }
    let tail = read_log_tail(&log_path).await;
    outcome.map_err(|source| {
        let source = match exit {
            Some(Ok(status)) => format!("{source} ({program} exited: {status})"),
            Some(Err(wait)) => format!("{source} ({program} wait failed: {wait})"),
            None => source,
        };
        if tail.is_empty() {
            source
        } else {
            format!("{source}: {tail}")
        }
    })
}

/// Spawn the agent from its plan with piped protocol stdio, returning the
/// child and its taken stdin/stdout. `stderr` is where the agent's log goes.
/// The agent gets all of `environment`, credentials included, since it signs
/// in and runs commands itself, then the plan's overrides (#1659).
pub(super) fn spawn_agent(
    program: &str,
    executable: &CliExecutable,
    plan: AcpSpawnPlan,
    stderr: Stdio,
    before_spawn: Option<&BeforeSpawn>,
    environment: &Ambient,
) -> Result<(OwnedChild, ChildStdin, ChildStdout), String> {
    let AcpSpawnPlan { argv, cwd, env } = plan;
    let mut command = executable
        .try_command(&argv)
        .map_err(|source| format!("{program}: {source}"))?;
    environment.apply_full(&mut command);
    command
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .kill_on_drop(true);
    if let Some(before_spawn) = before_spawn {
        before_spawn(&mut command).map_err(|source| {
            format!("{program}: frozen executable changed before spawn: {source}")
        })?;
    }
    let mut child = OwnedChild::spawn(command).map_err(|source| {
        format!(
            "{program}: failed to spawn `{}`: {source}",
            executable.display()
        )
    })?;
    let stdin = child
        .stdin()
        .ok_or_else(|| format!("{program}: missing child stdin"))?;
    let stdout = child
        .stdout()
        .ok_or_else(|| format!("{program}: missing child stdout"))?;
    Ok((child, stdin, stdout))
}

fn stopped() -> DriverOutcome {
    DriverOutcome {
        classification: TurnClassification::Stopped,
        session_id: None,
        result_text: String::new(),
        turns: 0,
    }
}

/// Every setup, transport, turn, or cancellation outcome reaches exactly one
/// terminal sink method. No driver branch writes a terminal artifact itself.
pub(super) async fn settle(mut sink: StatusSink, outcome: Result<DriverOutcome, String>) {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            sink.fail(error).await;
            return;
        }
    };
    let terminal = TerminalResult {
        result_text: outcome.result_text,
        session_id: outcome.session_id,
        turns: outcome.turns,
    };
    match (
        &outcome.classification,
        outcome.classification.failure_detail(),
    ) {
        (TurnClassification::Success, _) => sink.succeed(&terminal).await,
        (TurnClassification::Stopped, _) => sink.stop("cancelled", &terminal).await,
        (TurnClassification::Failure { .. } | TurnClassification::Invalid, detail) => {
            let detail = detail.expect("a failed turn has a detail");
            sink.fail_turn(&terminal, detail).await;
        }
    }
}

/// Same artifact boundary as production, replacing only the physical transport.
#[cfg(test)]
pub(super) async fn run_on_channel<P: AcpAgentPolicy>(
    mut request: AcpSessionRequest,
    mut policy: P,
    channel: agent_client_protocol::Channel,
) -> anyhow::Result<()> {
    let mut sink = open_sink(&mut request, &policy)?;
    let mut renderer = EventRenderer::new(request.cwd.clone());
    let outcome = match prepare(&request, &mut policy, &mut renderer, &mut sink) {
        Ok(prompt) => {
            driver::run(
                channel,
                policy,
                driver::DriverContext {
                    prompt: &prompt,
                    cwd: &request.cwd,
                    cancellation: &request.cancellation,
                    inbox: &mut request.parent_messages,
                    renderer: &mut renderer,
                    sink: &mut sink,
                },
            )
            .await
        }
        Err(error) => Err(error),
    };
    if let Some(inbox) = request.parent_messages.as_ref() {
        inbox.seal();
    }
    settle(sink, outcome).await;
    Ok(())
}
