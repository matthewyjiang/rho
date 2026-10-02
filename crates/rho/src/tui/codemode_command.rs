//! `/codemode [on|only]`: Pi's `codemode.mode` for the always-registered
//! `codemode` tool.
//!
//! `on` declares direct tools next to `codemode`; `only` hides them so the
//! model composes through `codemode`. Bare `/codemode` reports the mode. There
//! is no mode that removes `codemode` and no `yolo` level: nested calls
//! inherit the session permission mode, so `/permissions` is the only
//! permission ladder.

use crate::config::CodemodeMode;

use super::{App, CommandInvocation, Entry, InteractiveRuntime};

const CODEMODE_USAGE: &str = "usage: /codemode [on|only]";

/// Runtime side of `/codemode`. Implementors apply the mode to the next model
/// request and return transcript notice text when it changed.
pub(super) trait CodemodeRuntime {
    fn codemode_mode(&self) -> CodemodeMode;

    fn set_codemode_mode(&mut self, mode: CodemodeMode) -> anyhow::Result<Option<String>>;

    /// Live tool specs after a change, for the diagnostics mirror.
    fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec>;
}

impl CodemodeRuntime for InteractiveRuntime {
    fn codemode_mode(&self) -> CodemodeMode {
        InteractiveRuntime::codemode_mode(self)
    }

    fn set_codemode_mode(&mut self, mode: CodemodeMode) -> anyhow::Result<Option<String>> {
        InteractiveRuntime::set_codemode_mode(self, mode)
    }

    fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        InteractiveRuntime::tool_specs(self)
    }
}

impl App {
    pub(super) fn execute_codemode_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut impl CodemodeRuntime,
    ) -> anyhow::Result<()> {
        let current = agent.codemode_mode();
        if invocation.args.trim().is_empty() {
            self.report_codemode(current);
            return Ok(());
        }
        let Some(requested) = CodemodeMode::parse(&invocation.args) else {
            self.insert_entry(&Entry::Error(CODEMODE_USAGE.into()));
            self.set_status("invalid codemode mode");
            return Ok(());
        };

        let notice = match agent.set_codemode_mode(requested) {
            Ok(notice) => notice,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not apply codemode to this session: {error}"
                )));
                self.set_status("codemode change failed");
                return Ok(());
            }
        };
        let Some(display) = notice else {
            self.report_codemode(current);
            return Ok(());
        };
        // The runtime has already recorded the transition in model and durable
        // display history. Mirror it before saving the preference.
        self.insert_entry(&Entry::Notice(display));
        self.info
            .services
            .diagnostics
            .update_tools(&agent.tool_specs());
        if let Err(error) = self
            .info
            .services
            .config_repository
            .update(|config| config.codemode.mode = requested)
        {
            // Compensation records a reverse transition; it does not erase
            // the forward notice already visible to the model and the user.
            let detail = match agent.set_codemode_mode(current) {
                Ok(reverse) => {
                    if let Some(reverse) = reverse {
                        self.insert_entry(&Entry::Notice(reverse));
                    }
                    self.info
                        .services
                        .diagnostics
                        .update_tools(&agent.tool_specs());
                    format!("could not save codemode setting: {error}")
                }
                Err(compensation_error) => format!(
                    "could not save codemode setting: {error}; could not restore codemode mode: {compensation_error}"
                ),
            };
            self.insert_entry(&Entry::Error(detail));
            self.set_status("config save failed");
            return Ok(());
        }
        self.report_codemode(requested);
        Ok(())
    }

    fn report_codemode(&mut self, mode: CodemodeMode) {
        let status = match mode {
            CodemodeMode::On => "codemode on: direct tools and codemode are both available",
            CodemodeMode::Only => "codemode only: direct tools are reached through codemode",
        };
        self.set_status(status);
    }
}

#[cfg(test)]
#[path = "codemode_command_tests.rs"]
mod tests;
