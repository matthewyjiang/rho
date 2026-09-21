use super::{App, CommandInvocation, Entry, InteractiveRuntime};

trait FastModeRuntime {
    fn fast_mode(&self) -> bool;
    fn set_fast_mode(&self, enabled: bool) -> anyhow::Result<()>;
}

impl FastModeRuntime for InteractiveRuntime {
    fn fast_mode(&self) -> bool {
        self.fast_mode()
    }

    fn set_fast_mode(&self, enabled: bool) -> anyhow::Result<()> {
        self.set_fast_mode(enabled)
    }
}

impl App {
    pub(super) fn execute_fast_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider = self.info.runtime.provider.clone();
        let model = self.info.runtime.model.clone();
        let auth = self.info.runtime.auth.clone();
        // xAI prices the wire id, so a toggle has to reload that metadata.
        // Codex keeps the same model id and does not.
        let refresh_metadata = matches!(
            rho_providers::providers::fast_mode::fast_serving(&provider, &model, &auth),
            Some(rho_providers::providers::fast_mode::FastServing::RequestModel(_))
        );
        let changed = self.execute_fast_command_with_runtime(invocation, agent)?;
        if changed && refresh_metadata {
            self.start_model_metadata_fetch(agent);
        }
        Ok(())
    }

    fn execute_fast_command_with_runtime(
        &mut self,
        invocation: CommandInvocation,
        agent: &impl FastModeRuntime,
    ) -> anyhow::Result<bool> {
        let provider = &self.info.runtime.provider;
        let model = &self.info.runtime.model;
        let supported = rho_providers::providers::fast_mode::supports_fast_mode(
            provider,
            model,
            &self.info.runtime.auth,
        );
        let current = agent.fast_mode();
        let requested = match invocation.args.trim().to_ascii_lowercase().as_str() {
            "" => !current,
            "on" => true,
            "off" => false,
            _ => {
                self.insert_entry(&Entry::Error("usage: /fast [on|off]".into()));
                self.set_status("invalid fast mode");
                return Ok(false);
            }
        };

        if requested && !supported {
            let message =
                if provider == "xai" && model == rho_providers::providers::fast_mode::GROK_4_7 {
                    "fast mode for xai/grok-4.7 requires xAI OAuth".to_string()
                } else {
                    format!("fast mode is not available for {provider}/{model}")
                };
            self.insert_entry(&Entry::Error(message));
            self.set_status("fast mode unavailable");
            return Ok(false);
        }

        if requested != current {
            agent.set_fast_mode(requested)?;
            if let Err(error) = self
                .info
                .services
                .config_repository
                .update(|config| config.fast_mode = requested)
            {
                agent.set_fast_mode(current)?;
                self.insert_entry(&Entry::Error(format!("could not save fast mode: {error}")));
                self.set_status("config save failed");
                return Ok(false);
            }
            self.info.runtime.service_tier =
                requested.then_some(rho_sdk::model::ServiceTier::Priority);
            self.report_fast_mode(requested, supported);
            return Ok(true);
        }

        self.report_fast_mode(requested, supported);
        Ok(false)
    }

    fn report_fast_mode(&mut self, enabled: bool, supported: bool) {
        let (mode, detail) = match (enabled, supported) {
            (true, true) => ("on", "faster responses at a higher credit rate"),
            (true, false) => (
                "on",
                "inactive because the current model does not support it",
            ),
            (false, _) => ("off", "standard response speed and credit rate"),
        };
        self.set_status(format!("fast mode is {mode}: {detail}"));
    }
}

#[cfg(test)]
#[path = "fast_command_tests.rs"]
mod tests;
