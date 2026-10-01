//! `/codemode [on|off]`: offer or remove the `codemode` composition tool.
//!
//! There is deliberately no `yolo` level. Nested `call_tool` calls inherit the
//! session permission mode, so `/permissions` is the only permission ladder.

use std::future::Future;

use super::{App, CommandInvocation, Entry, InteractiveRuntime};

const CODEMODE_USAGE: &str = "usage: /codemode [on|off]";

/// Runtime side of `/codemode`. Implementors apply the change to the next turn
/// and return transcript notice text when the advertised tool list changed.
pub(super) trait CodemodeRuntime {
    fn codemode_enabled(&self) -> bool;

    fn set_codemode(
        &mut self,
        enabled: bool,
    ) -> impl Future<Output = anyhow::Result<Option<String>>> + Send;

    /// Live tool specs after a change, for the diagnostics mirror.
    fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec>;
}

impl CodemodeRuntime for InteractiveRuntime {
    fn codemode_enabled(&self) -> bool {
        InteractiveRuntime::codemode_enabled(self)
    }

    fn set_codemode(
        &mut self,
        enabled: bool,
    ) -> impl Future<Output = anyhow::Result<Option<String>>> + Send {
        InteractiveRuntime::set_codemode(self, enabled)
    }

    fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        InteractiveRuntime::tool_specs(self)
    }
}

impl App {
    pub(super) async fn execute_codemode_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.execute_codemode_command_with_runtime(invocation, agent)
            .await
    }

    async fn execute_codemode_command_with_runtime(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut impl CodemodeRuntime,
    ) -> anyhow::Result<()> {
        let current = agent.codemode_enabled();
        let requested = match invocation.args.trim().to_ascii_lowercase().as_str() {
            "" => !current,
            "on" => true,
            "off" => false,
            _ => {
                self.insert_entry(&Entry::Error(CODEMODE_USAGE.into()));
                self.set_status("invalid codemode mode");
                return Ok(());
            }
        };
        if requested == current {
            self.report_codemode(current);
            return Ok(());
        }

        let notice = match agent.set_codemode(requested).await {
            Ok(notice) => notice,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not apply codemode to this session: {error}"
                )));
                self.set_status("codemode change failed");
                return Ok(());
            }
        };
        if let Err(error) = self
            .info
            .services
            .config_repository
            .update(|config| config.codemode = requested)
        {
            // Keep runtime and saved preference aligned with the failed save.
            let rollback = agent.set_codemode(current).await;
            let detail = match rollback {
                Ok(_) => format!("could not save codemode: {error}"),
                Err(rollback_error) => format!(
                    "could not save codemode: {error}; runtime rollback failed: {rollback_error}"
                ),
            };
            self.insert_entry(&Entry::Error(detail));
            self.set_status("config save failed");
            return Ok(());
        }
        if let Some(display) = notice {
            self.insert_entry(&Entry::Notice(display));
            self.info
                .services
                .diagnostics
                .update_tools(&agent.tool_specs());
        }
        self.report_codemode(requested);
        Ok(())
    }

    fn report_codemode(&mut self, enabled: bool) {
        let status = if enabled {
            "codemode is on: nested tools follow the current permission mode"
        } else {
            "codemode is off"
        };
        self.set_status(status);
    }
}

#[cfg(test)]
#[path = "codemode_command_tests.rs"]
mod tests;
