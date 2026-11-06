//! A Rho agent run that a session or a workflow started: a subagent the
//! `agent` tool delegated, or a workflow agent node. Its session is saved
//! like any other, tagged with its role, unless its parent's is never saved,
//! and the parent watches its status file. The child sends its prompt as
//! given, decides its approvals alone or as the parent passed down, and
//! stops when the parent cancels it. A workflow node an earlier process left
//! running reopens its saved session and continues it instead.

use std::{path::PathBuf, sync::Arc};

use rho_sdk::UserInput;
use rho_tools::cancellation::RunCancellation;

use super::{run_prompt, HeadlessView, ParentQuestions, PromptEnd, RunReporter, Stops};
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
    session::SessionPersistence,
    subagent::RunStatus,
};

/// The session a child runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChildSession {
    /// A new session under this id: a subagent's, or a workflow node
    /// attempt's first run.
    New(rho_sdk::SessionId),
    /// The session saved under this id, reopened as the agent the child is
    /// bound to: a workflow node attempt that an earlier process left
    /// running. The child continues the conversation saved there, or sends
    /// its prompt when none was saved yet.
    Reopen(rho_sdk::SessionId),
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
    /// Whether a new session is saved: as its parent's is, for a subagent;
    /// always, for a workflow node. A reopened session is saved already.
    pub(crate) persistence: SessionPersistence,
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
    pub(crate) session: ChildSession,
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
        persistence,
        questions,
        notices,
        steering_slot,
        cancellation,
        status_tx,
        live_title,
        hook_host_labels,
        approvals,
        session,
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
        status_tx,
        live_title,
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
    let open = async {
        // A reopened session runs as the agent the child is bound to: a
        // child never rebinds with `/agent:`, so its file records a root
        // session, which reopens as the launch agent (see `loaded_agent`).
        let runtime = match session {
            ChildSession::New(id) => SessionRuntime::open_new(opening, id, persistence).await?,
            ChildSession::Reopen(id) => SessionRuntime::reopen(opening, id.as_str()).await?,
        };
        // The parent steers the child's model turns through its handle.
        if let Some(slot) = &steering_slot {
            slot.publish(runtime.handle());
        }
        anyhow::Ok(runtime)
    };
    let mut view = HeadlessView::new()
        .reporting_to(&mut reporter)
        .with_parent_questions(questions);
    // The child's status file keeps the title its prompt was given; the
    // session's own title request is left untaken.
    let end = run_prompt(
        &mut stops,
        &mut view,
        &config,
        open,
        |runtime, view, scope, process_config| {
            // A reopened session saved before its prompt reached the model,
            // or not at all, holds no conversation and still needs it.
            let start = if runtime.has_conversation() {
                TurnStart::Continue
            } else {
                let input = UserInput::text(prompt);
                TurnStart::Prompt(Turn {
                    display: input.clone(),
                    model: input,
                    calls: Vec::new(),
                })
            };
            Box::pin(runtime.run_prompt_turn(view, scope, start, process_config))
        },
    )
    .await;
    drop(view);
    match end {
        PromptEnd::Finished(outcome) => reporter.finish_ok(&outcome),
        PromptEnd::Stopped(_) => reporter.finish_stopped(),
        PromptEnd::Failed(error) => {
            reporter.finish_error(&error);
            return Err(error);
        }
    }
    Ok(())
}
