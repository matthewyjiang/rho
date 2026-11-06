//! A Rho agent run that a session or a workflow started: a subagent the
//! `agent` tool delegated, or a workflow agent node. Its session is saved
//! like any other, tagged with its role, and the parent watches its status
//! file. The child sends its prompt as given, decides its approvals alone
//! or as the parent passed down, and stops when the parent cancels it.

use std::{path::PathBuf, sync::Arc};

use rho_sdk::{CancellationToken, UserInput};
use rho_tools::cancellation::RunCancellation;

use super::{
    cancel_on_stop, until_stopped, HeadlessView, ParentQuestions, PromptEnd, RunReporter, Stops,
};
use crate::{
    app::{
        agent_binding::BoundAgent,
        cli_config,
        session_runtime::{
            HostCapabilities, Logouts, QuestionSupport, RunShape, SessionBuild, SessionLaunch,
            SessionOpening, SessionRole, SessionRuntime, Turn, TurnStart, UnattendedApprovals,
        },
        shutdown_signal::ShutdownSignals,
        subagent_messaging::{NoticePoster, SteeringSlot, CHILD_COMMUNICATION_CONTRACT},
    },
    config::Config,
    diagnostics::RuntimeDiagnostics,
    subagent::RunStatus,
};

/// Step checkpoints for one workflow node's run.
///
/// The session also saves to `store` at every step boundary. With `resume`,
/// the run restores that snapshot and continues it instead of sending the
/// prompt.
#[derive(Clone)]
pub(crate) struct SessionCheckpoint {
    pub(crate) store: Arc<dyn rho_sdk::SessionStore>,
    pub(crate) resume: Option<rho_sdk::SessionSnapshot>,
}

/// One child run on the Rho runtime, after binding and permit acquisition.
pub(crate) struct ChildRun {
    pub(crate) bound: BoundAgent,
    pub(crate) config: Config,
    pub(crate) config_path: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) prompt: String,
    pub(crate) output_file: PathBuf,
    /// A subagent with its parent, or a workflow node.
    pub(crate) role: SessionRole,
    /// The parent that answers the child's questions, when one listens.
    pub(crate) questions: Option<ParentQuestions>,
    /// The parent the child posts notices to, when one listens.
    pub(crate) notices: Option<Arc<dyn NoticePoster>>,
    pub(crate) steering_slot: Option<SteeringSlot>,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: tokio::sync::watch::Sender<RunStatus>,
    pub(crate) live_title: crate::run_artifacts::LiveRunTitle,
    pub(crate) hook_host_labels: rho_sdk::hooks::HookHostLabels,
    pub(crate) approvals: UnattendedApprovals,
    pub(crate) checkpoint: Option<SessionCheckpoint>,
    pub(crate) added_dirs: crate::added_dirs::AddedDirs,
    pub(crate) host_io: crate::tools::host_io::HostIo,
}

/// Runs `run` to its end, reporting it in its status file. Fails only when
/// the run could not be reported or failed; a stop ends it as stopped.
pub(crate) async fn run_child(run: ChildRun) -> anyhow::Result<()> {
    let ChildRun {
        bound,
        mut config,
        config_path,
        cwd,
        prompt,
        output_file,
        role,
        questions,
        notices,
        steering_slot,
        cancellation,
        status_tx,
        live_title,
        hook_host_labels,
        approvals,
        checkpoint,
        added_dirs,
        host_io,
    } = run;

    cli_config::prepare_model_metadata(
        &config,
        &crate::credential_store::AppCredentialStore,
        &cli_config::ProviderRefreshStatus::NotAttempted,
    )
    .await;
    cli_config::normalize_reasoning(&mut config);
    let diagnostics = RuntimeDiagnostics::new(&config);
    diagnostics.update_agent(bound.id().as_str(), &bound.fingerprint().to_string());
    let started_status = status_tx.borrow().clone();
    let mut reporter = RunReporter::continue_from(
        output_file,
        started_status,
        cwd.clone(),
        &prompt,
        Some(status_tx),
        Some(live_title),
        steering_slot.clone(),
    )?;
    let max_steps = std::num::NonZeroUsize::new(
        bound
            .step_limit()
            .try_into()
            .map_err(|_| anyhow::anyhow!("agent step limit does not fit this platform"))?,
    )
    .ok_or_else(|| anyhow::anyhow!("agent step limit must be positive"))?;
    let mut stops = Stops::on_signals(ShutdownSignals::install()?).or_cancelled(cancellation);
    let build = SessionBuild {
        launch: SessionLaunch {
            config_path,
            no_system_prompt: false,
            no_tools: false,
            no_subagents: true,
            diagnostics,
            agent: bound,
            prompt_suffix: Some(CHILD_COMMUNICATION_CONTRACT),
        },
        host: HostCapabilities {
            questions: match &questions {
                Some(_) => QuestionSupport::Available,
                None => QuestionSupport::Unavailable,
            },
            host_io,
            parent_notices: notices,
            ..HostCapabilities::unattended(approvals)
        },
        run: RunShape {
            role,
            max_steps: Some(max_steps),
            hook_labels: hook_host_labels,
        },
    };
    let logouts = Logouts::default();
    let opening = SessionOpening {
        build,
        process_config: &config,
        host_mcp: Default::default(),
        cwd: &cwd,
        added_dirs,
        logouts: &logouts,
    };
    let (store, resume) = match checkpoint {
        Some(SessionCheckpoint { store, resume }) => (Some(store), resume),
        None => (None, None),
    };
    let mut view = HeadlessView::new(Some(&mut reporter), /*jsonl*/ None)
        .with_steering(steering_slot)
        .with_parent_questions(questions);
    let stop = CancellationToken::new();
    let work = async {
        let (mut runtime, start) = match resume {
            Some(snapshot) => (
                SessionRuntime::open_from_checkpoint(opening, snapshot).await?,
                TurnStart::Continue,
            ),
            None => {
                let input = UserInput::text(prompt);
                let turn = Turn {
                    display: input.clone(),
                    model: input,
                    calls: Vec::new(),
                };
                (
                    SessionRuntime::open_new(opening, rho_sdk::SessionId::new()).await?,
                    turn.into(),
                )
            }
        };
        if let Some(store) = store {
            runtime.mirror_step_checkpoints(store);
        }
        view.bind(&runtime);
        let ended = cancel_on_stop(&stop, runtime.handle(), async {
            let scope = runtime.begin_prompt(&config).await?;
            runtime
                .run_prompt_turn(&mut view, &scope, start, &config)
                .await
        })
        .await;
        // The child's status file keeps the title its prompt was given; the
        // session's own title request is left untaken.
        runtime.shutdown().await;
        ended
    };
    let (ended, cause) = until_stopped(&mut stops, &stop, work).await;
    drop(view);
    match PromptEnd::of(ended, cause) {
        PromptEnd::Finished(outcome) => reporter.finish_ok(&outcome),
        PromptEnd::Stopped(_) => reporter.finish_stopped(),
        PromptEnd::Failed(error) => {
            reporter.finish_error(&error);
            return Err(error);
        }
    }
    Ok(())
}
