//! The `/hooks` command: reload hook configuration and show the spawn contract.

use super::{App, Entry, InteractiveRuntime};

impl App {
    pub(super) fn execute_hooks_command(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match agent.reload_hooks() {
            Ok(report) => {
                self.insert_entry(&Entry::Notice(report.render()));
                self.status = if report.hooks.is_empty() {
                    "no hooks loaded".into()
                } else {
                    format!("{} hooks loaded", report.hooks.len())
                };
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not reload hooks: {error}")));
                self.status = "hook reload failed".into();
            }
        }
        Ok(())
    }
}
