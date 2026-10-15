//! `/workflow:<name>` for ACP sessions: start a workspace workflow in the
//! background and end the prompt without a model turn.
//!
//! ACP gives the agent no way to start a turn on its own, so the run's result
//! cannot be announced when it finishes. It reaches the model at the first
//! boundary of a later prompt, through the same headless delivery as other
//! background work. The run id goes into context now, so the model can check
//! on it with the `workflow` tool in the meantime.

use std::path::Path;

use agent_client_protocol::schema::v1::{PromptResponse, StopReason};
use rho_sdk::{model::Message, CancellationToken, UserInput};

use super::{send_notification, EventMapper, SessionHost};
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::{
        acp::AcpClientPort,
        workflow_cli::{self, discover::StartCommand, WorkflowOps},
        workflow_runtime::RecoveryDecision,
    },
    config::Config,
    tools::workflow_tracker::start_context_prompts,
};

impl SessionHost {
    /// Plans `start`, launches its run, records the run id in context, and
    /// saves the prompt. `display` is the command as the user sent it.
    ///
    /// A cancel stops planning. Once the run exists it keeps going: cancel
    /// it with the `workflow` tool or the CLI, like any background run.
    pub(super) async fn start_workflow(
        &mut self,
        display: &UserInput,
        start: StartCommand,
        process_config: &Config,
        config_path: &Path,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let source = start.workflow.relative_path.clone();
        let failed = |error: anyhow::Error| anyhow::anyhow!("could not start {source}: {error:#}");
        let cancel = CancellationToken::new();
        self.prompt_gate.activate(cancel.clone());

        let ops = WorkflowOps::open(
            self.stored.cwd().to_path_buf(),
            Some(config_path.to_path_buf()),
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
            () = cancel.cancelled() => return Ok(PromptResponse::new(StopReason::Cancelled)),
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
            Some(config_path.to_path_buf()),
            Some(tracker),
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("could not start workflow {workflow_name} in the background: {error:#}")
        })?;

        let (model, notice) = start_context_prompts(&run_id, &workflow_name, &program_digest);
        self.record_started(display, model, &notice)?;
        send_notification(
            client,
            EventMapper::agent_notice(&self.acp_session_id, &notice),
        )
        .await?;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    /// Adds the run id to model context and saves the prompt with `notice` as
    /// its reply. A failed save rolls the context back so the live session
    /// never holds history its file lacks. No SDK run happens here, so no turn
    /// checkpoint is involved; a replaced host leaves the file to its successor.
    fn record_started(
        &self,
        display: &UserInput,
        model: String,
        notice: &str,
    ) -> anyhow::Result<()> {
        let session = &self.built.session;
        let before = session.history();
        session.append_message(Message::user_text(model))?;
        if self.is_replaced() {
            return Ok(());
        }
        let display_rows = [
            Message::User(display.blocks().to_vec()),
            Message::assistant_text(notice),
        ];
        if let Err(error) = self
            .stored
            .save_snapshot(&self.durable_snapshot(), &display_rows)
        {
            session.replace_history(before)?;
            // ACP shows only the outermost message, so keep the cause inline.
            anyhow::bail!("workflow started, but could not save its run id: {error:#}");
        }
        Ok(())
    }
}
