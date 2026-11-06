//! The `--output-file` status contract of a headless run: `rho run
//! --output-file` and every delegated child keep a status file and event log
//! a parent watches, and `rho run` streams progress to stdout so a watching
//! pane shows live activity.

use std::{
    io::{self, Write},
    path::PathBuf,
};

use crate::{
    app::subagent_messaging::SteeringSlot,
    run_artifacts::RunArtifactIdentity,
    subagent::{RunState, RunStatus},
};

/// Keeps one run's status file and event log as its events arrive.
pub(crate) struct RunReporter {
    sink: crate::run_artifacts::RunArtifactSink,
    adapter: crate::tui::event_adapter::SdkEventAdapter,
    stream_output: bool,
    steering_slot: Option<SteeringSlot>,
}

impl RunReporter {
    /// Starts `rho run --output-file`'s status file at `path`, which no
    /// parent in this process watches.
    pub(crate) fn new(
        path: PathBuf,
        identity: RunArtifactIdentity,
        cwd: PathBuf,
        prompt: &str,
        stream_output: bool,
    ) -> anyhow::Result<Self> {
        let sink = crate::run_artifacts::RunArtifactSink::open(
            path, &identity, prompt, /*status_tx*/ None,
        )?;
        Ok(Self {
            sink,
            // MCP tools connect with the session, after this reporter exists.
            adapter: crate::tui::event_adapter::SdkEventAdapter::new(
                cwd,
                crate::tools::mcp::exported_name::McpToolNames::default(),
            ),
            stream_output,
            steering_slot: None,
        })
    }

    /// Resume after the executor already wrote the Starting boundary,
    /// publishing each status to the parent's `status_tx` and taking the
    /// title the parent names the run with from `live_title`.
    pub(crate) fn continue_from(
        path: PathBuf,
        started_status: RunStatus,
        cwd: PathBuf,
        prompt: &str,
        status_tx: tokio::sync::watch::Sender<RunStatus>,
        live_title: crate::run_artifacts::LiveRunTitle,
        steering_slot: Option<SteeringSlot>,
    ) -> anyhow::Result<Self> {
        let sink = crate::run_artifacts::RunArtifactSink::continue_from(
            path,
            started_status,
            prompt,
            Some(status_tx),
            Some(live_title),
        )?;
        Ok(Self {
            sink,
            adapter: crate::tui::event_adapter::SdkEventAdapter::new(
                cwd,
                crate::tools::mcp::exported_name::McpToolNames::default(),
            ),
            stream_output: false,
            steering_slot,
        })
    }

    /// Identify the session's MCP calls on progress cards once they connect.
    pub(super) fn set_mcp_tools(
        &mut self,
        mcp_tools: crate::tools::mcp::exported_name::McpToolNames,
    ) {
        self.adapter.set_mcp_tools(mcp_tools);
    }

    pub(super) fn on_event(&mut self, event: &rho_sdk::RunEvent) {
        use rho_sdk::RunEvent;

        if let rho_sdk::RunEvent::SteeringApplied { ids } = event {
            if let Some(slot) = &self.steering_slot {
                for message in slot.applied(ids) {
                    self.sink
                        .record_attachment(crate::run_artifacts::AttachmentEvent::Message(
                            Box::new(crate::presentation::parent_message_card(
                                message.into_string(),
                                crate::presentation::NotificationDelivery::Received,
                                "applied to conversation history".into(),
                            )),
                        ));
                }
            }
        }
        let attachments = crate::tui::translate_run_event(&mut self.adapter, event);
        for attachment in attachments {
            // Reasoning is deliberately kept out of `last_text`: the status file
            // carries the answer, not the thinking.
            if let crate::run_artifacts::AttachmentEvent::AssistantTextDelta(text) = &attachment {
                if !text.is_empty() {
                    self.sink.append_last_text(text);
                }
            }
            self.sink.record_attachment(attachment);
        }
        match event {
            RunEvent::StepStarted { step, .. } => {
                self.sink.status.state = RunState::Running;
                self.sink.status.turns = *step as u64;
                self.sink.publish();
            }
            RunEvent::ToolStarted { name, .. } => {
                self.sink.status.last_activity = Some(format!("tool: {name}"));
                self.stream(&format!("\n[tool] {name}\n"));
                self.sink.publish();
            }
            RunEvent::HostInputRequested { request }
            | RunEvent::ToolHostInputRequested { request, .. } => {
                self.sink.status.last_activity =
                    Some(format!("waiting for questionnaire: {}", request.title()));
                self.sink.publish();
            }
            RunEvent::AssistantTextDelta { text } => {
                self.sink.status.last_activity = Some("assistant text".into());
                self.stream(text);
                // Attachment path already published throttled when translated.
            }
            RunEvent::ProviderStreamReset { .. } => {
                self.sink.status.last_activity = Some("retrying provider response".into());
                self.sink.status.last_text = None;
                self.stream("\n[provider response discarded; retrying]\n");
                self.sink.publish();
            }
            RunEvent::UsageUpdated { usage } => {
                self.sink.status.input_tokens = usage.inclusive_prompt_tokens();
                self.sink.status.output_tokens = usage.output_tokens;
            }
            _ => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> &RunStatus {
        &self.sink.status
    }

    pub(super) fn write(&mut self) {
        self.sink.publish();
    }

    /// Ends the status as `ok`, with the run's answer and usage.
    pub(crate) fn finish_ok(&mut self, outcome: &rho_sdk::RunOutcome) {
        let usage = outcome.usage();
        self.sink.status.input_tokens = usage.inclusive_prompt_tokens();
        self.sink.status.output_tokens = usage.output_tokens;
        self.sink.finish_ok(Some(outcome.text().to_string()));
    }

    /// Ends the status as stopped: by a stop, or at `rho run`'s step limit.
    pub(crate) fn finish_stopped(&mut self) {
        self.sink.finish_stopped("stopped");
    }

    pub(crate) fn finish_error(&mut self, error: &anyhow::Error) {
        self.sink.finish_error(format!("{error:#}"));
    }

    fn stream(&self, text: &str) {
        if !self.stream_output {
            return;
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }
}

#[cfg(test)]
#[path = "reporter_tests.rs"]
mod tests;
