//! `/workflow:<name>`, and the hub's `/workflow:plan/<id>` and
//! `/workflow:run/<id>`: start a workflow run in the background and end the
//! prompt without a model turn.
//!
//! Hosts such as ACP give the agent no way to start a turn on its own, so the
//! run's result cannot be announced when it finishes. It reaches the model at the first
//! boundary of a later prompt, through the same headless delivery as other
//! background work. A host that starts turns itself, as the daemon does for
//! the TUI, delivers it in a turn of its own once the run finishes. The run
//! id goes into context now, so the model can check on it with the
//! `workflow` tool in the meantime.

use std::path::Path;

use rho_sdk::{model::Message, CancellationToken, UserInput};

use super::{prompt_gate::ActiveWork, InsertedMessage, PromptScope, SessionRuntime};
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::{
        workflow_cli::{self, discover::WorkflowLaunch, WorkflowOps},
        workflow_runtime::RecoveryDecision,
    },
    config::Config,
    tools::workflow_tracker::{start_context_prompts, starting_notice, BackgroundStart},
};

/// How a `/workflow:` prompt ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkflowStart {
    /// Cancelled while planning; no run exists.
    Cancelled,
    /// The run is going in the background; `notice` is the reply the prompt
    /// was saved with, for the host to show, `inserted` the prompt's user
    /// message as saved, if the person sent it, and `reply` the notice as
    /// saved.
    Started {
        notice: String,
        inserted: Option<InsertedMessage>,
        reply: InsertedMessage,
    },
}

/// Who sent a `/workflow:` prompt.
pub(crate) enum StartedBy<'a> {
    /// The person, whose prompt, `display` as they sent it, is saved as
    /// their message.
    Person { display: &'a UserInput },
    /// A host on the person's behalf, as a workflow hub: the start saves no
    /// user message.
    Host,
}

impl SessionRuntime {
    /// Starts `launch`'s run within `prompt`, records the run id in
    /// context, and saves the prompt as `by` says.
    ///
    /// A cancel stops a source's planning; a saved plan and a run have none
    /// to stop. Once the run exists it keeps going: cancel
    /// it with the `workflow` tool or the CLI, like any background run.
    pub(crate) async fn start_workflow(
        &mut self,
        prompt: &PromptScope,
        by: StartedBy<'_>,
        launch: WorkflowLaunch,
        process_config: &Config,
    ) -> anyhow::Result<WorkflowStart> {
        let cancel = CancellationToken::new();
        prompt.activate(ActiveWork::command(cancel.clone()));

        // One snapshot for planning and the run: the run must verify its
        // frozen executables on the `PATH` planning found them on (#1659).
        let environment = self.build.host.environment.snapshot();
        let ops = WorkflowOps::open(
            self.storage.cwd().to_path_buf(),
            Some(self.build.launch.config_path.clone()),
            &environment,
        )
        .map_err(|error| anyhow::anyhow!("could not open the workflow store: {error:#}"))?;
        let plan_id;
        let (run, recovery, start) = match launch {
            WorkflowLaunch::Source(start) => {
                let default_inputs = start.inputs.is_empty();
                let source = start.workflow.relative_path.clone();
                let failed =
                    |error: anyhow::Error| anyhow::anyhow!("could not start {source}: {error:#}");
                // The same ceiling the session's own workflow tool gives
                // its runs.
                let available_tools = AgentCapabilities::new(
                    self.built
                        .tools
                        .unfiltered_names()
                        .map(ToolCapability::parse)
                        .collect(),
                );
                let prepared = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Ok(WorkflowStart::Cancelled),
                    prepared = ops.prepare_local_json(
                        Path::new(&start.workflow.relative_path),
                        start.inputs,
                        process_config,
                        &available_tools,
                    ) => prepared.map_err(failed)?,
                };
                let run = ops.create_launch_run(&prepared).map_err(failed)?;
                (
                    run,
                    RecoveryDecision::NormalResume,
                    BackgroundStart::Source { default_inputs },
                )
            }
            WorkflowLaunch::Plan(plan) => {
                let stored = ops
                    .prepare_run_id(plan)
                    .map_err(|error| anyhow::anyhow!("could not prepare plan: {error:#}"))?;
                let run = ops
                    .create_confirmed_run(&stored)
                    .map_err(|error| anyhow::anyhow!("could not create run: {error:#}"))?;
                plan_id = plan.to_string();
                (
                    run,
                    RecoveryDecision::NormalResume,
                    BackgroundStart::Plan { plan_id: &plan_id },
                )
            }
            WorkflowLaunch::Resume {
                run,
                recover_uncertain,
            } => {
                let run = ops
                    .load_run_id(run)
                    .map_err(|error| anyhow::anyhow!("could not load run: {error:#}"))?;
                let recovery = ops
                    .prepare_resume(&run, recover_uncertain)
                    .map_err(|error| anyhow::anyhow!("could not open run: {error:#}"))?;
                (run, recovery, BackgroundStart::Resume)
            }
        };

        let run_id = run.manifest.run_id.to_string();
        let workflow_name = run.graph.program.name.as_str().to_owned();
        let program_digest = run.manifest.program_digest.0.clone();
        let tracker = self.built.tools.workflow_tracker().clone();
        if !tracker.register_start(
            run_id.clone(),
            workflow_name.clone(),
            program_digest.clone(),
            Some(self.built.session.id().to_string()),
        ) {
            anyhow::bail!(
                "could not start workflow {workflow_name}: run {run_id} is already running"
            );
        }
        workflow_cli::spawn_background_run(
            run,
            recovery,
            workflow_cli::BackgroundRun {
                cwd: self.storage.cwd().to_path_buf(),
                config_path: Some(self.build.launch.config_path.clone()),
                pool: self.build.host.subagent_pool.clone(),
                tracker,
                environment,
            },
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("could not start workflow in the background: {error:#}")
        })?;

        let (model, started) = start_context_prompts(&run_id, &workflow_name, &program_digest);
        let notice = format!(
            "{}\n\n{started}",
            starting_notice(&start, &workflow_name, &run_id)
        );
        let (inserted, reply) = self.record_started(by, model, &notice)?;
        Ok(WorkflowStart::Started {
            notice,
            inserted,
            reply,
        })
    }

    /// Adds the run id to model context and saves `notice`, after the
    /// person's prompt when `by` is the person, returning the prompt's user
    /// message and the notice as saved (see [`Self::append_context`]).
    fn record_started(
        &mut self,
        by: StartedBy<'_>,
        model: String,
        notice: &str,
    ) -> anyhow::Result<(Option<InsertedMessage>, InsertedMessage)> {
        let saved = match by {
            StartedBy::Person { display } => self
                .append_context(
                    model,
                    Message::User(display.blocks().into()),
                    [Message::assistant_text(notice)],
                )
                .map(|(inserted, [reply])| (Some(inserted), reply)),
            StartedBy::Host => self
                .append_context(model, Message::assistant_text(notice), [])
                .map(|(reply, [])| (None, reply)),
        };
        // Hosts such as ACP show only the outermost message, so keep the
        // cause inline.
        saved.map_err(|error| {
            anyhow::anyhow!("workflow started, but could not save its run id: {error:#}")
        })
    }
}
