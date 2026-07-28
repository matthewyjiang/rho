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
        self.execute_fast_command_with_runtime(invocation, agent)
    }

    fn execute_fast_command_with_runtime(
        &mut self,
        invocation: CommandInvocation,
        agent: &impl FastModeRuntime,
    ) -> anyhow::Result<()> {
        let provider = &self.info.runtime.provider;
        let model = &self.info.runtime.model;
        let supported = rho_providers::providers::openai::supports_fast_mode(provider, model);
        let current = agent.fast_mode();
        let requested = match invocation.args.trim().to_ascii_lowercase().as_str() {
            "" => !current,
            "on" => true,
            "off" => false,
            "status" => {
                self.report_fast_mode(current, supported);
                return Ok(());
            }
            _ => {
                self.insert_entry(&Entry::Error("usage: /fast [on|off|status]".into()));
                self.status = "invalid fast mode".into();
                return Ok(());
            }
        };

        if requested && !supported {
            self.insert_entry(&Entry::Error(format!(
                "fast mode is not available for {provider}/{model}"
            )));
            self.status = "fast mode unavailable".into();
            return Ok(());
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
                self.status = "config save failed".into();
                return Ok(());
            }
        }

        self.report_fast_mode(requested, supported);
        Ok(())
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
        self.insert_entry(&Entry::Notice(format!("fast mode is {mode}: {detail}")));
        self.status = format!("fast mode {mode}");
    }
}

#[cfg(test)]
#[path = "fast_command_tests.rs"]
mod tests;
