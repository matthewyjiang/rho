//! The `/mcp` command: show configured servers and session load status.

use super::{mcp_picker, App, ComposerMode};

impl App {
    pub(super) fn execute_mcp_command(&mut self) -> anyhow::Result<()> {
        let config_path = self.info.services.config_repository.configured_path()?;
        let picker = mcp_picker::picker(mcp_picker::McpPickerContext {
            report: &self.mcp_report,
            catalog: &self.mcp_catalog,
            config_path: &config_path,
        });
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("mcp servers");
        Ok(())
    }
}
