use std::future::Future;

use crate::{agent::ADVISOR_AGENT_ID, config::InternalAgentModelConfig};

use super::{
    advisor_status::AdvisorStatus, agent_picker::InternalAgentModelPickerOrigin, App,
    CommandInvocation, ComposerMode, Entry, InteractiveRuntime,
};

const SELECT_ADVISOR_MODEL_STATUS: &str = "select an advisor model to turn advisor mode on";

/// The runtime side of advisor mode.
///
/// Advisor mode changes the tool list and the system prompt, so the runtime has
/// to act on it rather than only save it. Implementors apply the change to the
/// next turn and leave the current session ID and history alone.
pub(super) trait AdvisorRuntime {
    /// Points the advisor at `model`, or turns the advisor off with `None`.
    fn set_advisor(
        &mut self,
        model: Option<InternalAgentModelConfig>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
}

impl AdvisorRuntime for InteractiveRuntime {
    fn set_advisor(
        &mut self,
        model: Option<InternalAgentModelConfig>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        InteractiveRuntime::set_advisor(self, model)
    }
}

impl App {
    pub(super) async fn execute_advisor_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.execute_advisor_command_with_runtime(invocation, agent)
            .await
    }

    async fn execute_advisor_command_with_runtime(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut impl AdvisorRuntime,
    ) -> anyhow::Result<()> {
        let requested = match invocation.args.trim().to_ascii_lowercase().as_str() {
            "" => !self.info.runtime.advisor_mode,
            "on" => true,
            "off" => false,
            _ => {
                self.insert_entry(&Entry::Error("usage: /advisor [on|off]".into()));
                self.set_status("invalid advisor mode");
                return Ok(());
            }
        };

        // Advisor mode on with no model does nothing, so `/advisor on` asks for
        // one instead of reporting a mode that cannot run.
        if requested && !self.advisor_model_configured() {
            self.open_advisor_model_prompt(InternalAgentModelPickerOrigin::AdvisorCommand);
            return Ok(());
        }
        self.set_advisor_mode(requested, agent).await
    }

    pub(super) fn advisor_model_configured(&self) -> bool {
        self.info
            .runtime
            .internal_agents
            .contains_key(ADVISOR_AGENT_ID)
    }

    /// Opens the advisor model picker. The origin places it: alone in the
    /// composer for `/advisor on`, under the config picker for its row, so
    /// escaping returns where the user came from.
    pub(super) fn open_advisor_model_prompt(&mut self, origin: InternalAgentModelPickerOrigin) {
        if self.open_internal_agent_model_picker(ADVISOR_AGENT_ID, origin) {
            self.set_status(SELECT_ADVISOR_MODEL_STATUS);
        }
    }

    /// Turns advisor mode on once the advisor model picker has stored a model.
    pub(super) async fn finish_advisor_model_selection(
        &mut self,
        selected: bool,
        agent: &mut impl AdvisorRuntime,
    ) -> anyhow::Result<()> {
        if !selected {
            return Ok(());
        }
        self.set_advisor_mode(true, agent).await
    }

    /// Drops a pending `/advisor on` model prompt. Reports whether one was open
    /// so the caller can leave its own dismissal status alone.
    pub(super) fn cancel_advisor_model_prompt(&mut self) -> bool {
        let pending = matches!(
            self.internal_agent_model_target.as_ref(),
            Some(target) if target.origin == InternalAgentModelPickerOrigin::AdvisorCommand
        );
        if pending {
            self.internal_agent_model_target = None;
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("advisor mode stays off: no advisor model selected");
        }
        pending
    }

    pub(super) async fn set_advisor_mode(
        &mut self,
        enabled: bool,
        agent: &mut impl AdvisorRuntime,
    ) -> anyhow::Result<()> {
        if self.info.runtime.advisor_mode != enabled {
            if let Err(error) = self
                .info
                .services
                .config_repository
                .update(|config| config.advisor_mode = enabled)
            {
                self.insert_entry(&Entry::Error(format!(
                    "could not save advisor mode: {error}"
                )));
                self.set_status("config save failed");
                return Ok(());
            }
            self.info.runtime.advisor_mode = enabled;
        }
        self.sync_advisor_runtime(agent).await;
        let status = self.advisor_mode_status();
        self.set_status(status);
        Ok(())
    }

    /// Applies the saved advisor state to the live runtime.
    ///
    /// The advisor model and the mode reach the runtime as one value, because
    /// advisor mode without a model offers the executor nothing.
    pub(super) async fn sync_advisor_runtime(&mut self, agent: &mut impl AdvisorRuntime) {
        let model = self.info.runtime.advisor_mode.then(|| {
            self.info
                .runtime
                .internal_agents
                .get(ADVISOR_AGENT_ID)
                .cloned()
        });
        self.info
            .services
            .diagnostics
            .update_advisor_mode(self.info.runtime.advisor_mode);
        if let Err(error) = agent.set_advisor(model.flatten()).await {
            self.insert_entry(&Entry::Error(format!(
                "advisor mode could not be applied to this session: {error}"
            )));
        }
    }

    pub(super) fn advisor_mode_status(&self) -> String {
        match AdvisorStatus::from_runtime(&self.info.runtime) {
            AdvisorStatus::Off => "advisor mode is off".into(),
            AdvisorStatus::Reviewing { model } => {
                format!("advisor mode is on: {model} reviews the session")
            }
            AdvisorStatus::MissingModel => {
                "advisor mode is on, but no advisor model is selected".into()
            }
        }
    }
}

#[cfg(test)]
#[path = "advisor_command_tests.rs"]
mod tests;
