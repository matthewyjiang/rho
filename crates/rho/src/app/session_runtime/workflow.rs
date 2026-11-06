//! `/workflow:<name>`: start a workspace workflow in the background and end
//! the prompt without a model turn.
//!
//! Hosts such as ACP give the agent no way to start a turn on its own, so the
//! run's result cannot be announced when it finishes. It reaches the model at the first
//! boundary of a later prompt, through the same headless delivery as other
//! background work. The run id goes into context now, so the model can check
//! on it with the `workflow` tool in the meantime.

use rho_sdk::{model::Message, CancellationToken, UserInput};

use super::{InsertedMessage, PromptScope, SessionRuntime};
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::{
        workflow_cli::{self, discover::StartCommand, WorkflowOps},
        workflow_runtime::RecoveryDecision,
    },
    config::Config,
    tools::workflow_tracker::start_context_prompts,
};

/// How a `/workflow:<name>` prompt ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkflowStart {
    /// Cancelled while planning; no run exists.
    Cancelled,
    /// The run is going in the background; `notice` is the reply the prompt
    /// was saved with, for the host to show, and `inserted` the prompt's
    /// user message as saved.
    Started {
        notice: String,
        inserted: InsertedMessage,
    },
}

impl SessionRuntime {
    /// Plans `start` within `prompt`, launches its run, records the run id
    /// in context, and saves the prompt. `display` is the command as the
    /// user sent it.
    ///
    /// A cancel stops planning. Once the run exists it keeps going: cancel
    /// it with the `workflow` tool or the CLI, like any background run.
    pub(crate) async fn start_workflow(
        &mut self,
        prompt: &PromptScope,
        display: &UserInput,
        start: StartCommand,
        process_config: &Config,
    ) -> anyhow::Result<WorkflowStart> {
        let source = start.workflow.relative_path.clone();
        let failed = |error: anyhow::Error| anyhow::anyhow!("could not start {source}: {error:#}");
        let cancel = CancellationToken::new();
        prompt.activate(cancel.clone());

        let ops = WorkflowOps::open(
            self.storage.cwd().to_path_buf(),
            Some(self.build.launch.config_path.clone()),
        )
        .map_err(failed)?;
        // The same ceiling the session's own workflow tool gives its runs.
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
                &start.workflow.absolute_path,
                start.inputs,
                process_config,
                &available_tools,
            ) => prepared.map_err(failed)?,
        };
        let run = ops.create_launch_run(&prepared).map_err(failed)?;

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
            anyhow::bail!("could not start {source}: run {run_id} is already running");
        }
        workflow_cli::spawn_background_run(
            run,
            RecoveryDecision::NormalResume,
            Some(self.build.launch.config_path.clone()),
            Some(tracker),
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("could not start workflow {workflow_name} in the background: {error:#}")
        })?;

        let (model, notice) = start_context_prompts(&run_id, &workflow_name, &program_digest);
        let inserted = self.record_started(display, model, &notice)?;
        Ok(WorkflowStart::Started { notice, inserted })
    }

    /// Adds the run id to model context and saves the prompt with `notice` as
    /// its reply, returning the prompt's user message as saved. A failed
    /// save rolls the context back so the live session never holds history
    /// its file lacks. No SDK run happens here, so no turn checkpoint is
    /// involved; a replaced runtime leaves the file to its successor.
    fn record_started(
        &mut self,
        display: &UserInput,
        model: String,
        notice: &str,
    ) -> anyhow::Result<InsertedMessage> {
        let session = self.built.session.clone();
        let before = session.history();
        session.append_message(Message::user_text(model))?;
        let saved = self.save_prompt_rows(
            Message::User(display.blocks().to_vec()),
            &[Message::assistant_text(notice)],
        );
        if let Err(error) = &saved {
            session.replace_history(before)?;
            // Hosts such as ACP show only the outermost message, so keep the
            // cause inline.
            anyhow::bail!("workflow started, but could not save its run id: {error:#}");
        }
        saved
    }
}
