//! The `/mcp` command: show configured servers and session load status.

use super::{mcp_picker, App, ComposerMode, StatusSource};

impl App {
    pub(super) fn show_daemon_mcp(
        &mut self,
        result: Result<Box<rho_protocol::SessionMcpInventory>, String>,
    ) {
        if self.apply_doctor_mcp(result.as_deref().map_err(String::as_str)) {
            return;
        }
        let open = matches!(self.input_ui.composer(), ComposerMode::Picker(picker) if picker.is_mcp_inventory());
        match result {
            Ok(inventory) if open => {
                self.input_ui
                    .set_composer(ComposerMode::Picker(mcp_picker::picker(&inventory)));
                self.set_status("mcp servers");
            }
            Ok(_) => {} // A late response must not reopen a dismissed picker.
            Err(error) => {
                if open {
                    self.input_ui.set_composer(ComposerMode::Input);
                }
                self.insert_entry(&super::Entry::Error(error));
            }
        }
    }

    /// Whether the session's tool sources still load, so a prompt waits.
    pub(super) fn tool_sources_pending(&self) -> bool {
        self.info.session.daemon.tool_sources_loading()
    }

    /// The session's MCP servers stopped connecting: held prompts may go
    /// and an open `/mcp` shows where the servers ended up. The session
    /// already counts them loaded by the time it sends the view.
    pub(super) fn show_daemon_tool_sources_loaded(&mut self) {
        if self.status_source == StatusSource::ToolSourcesLoading {
            self.set_status_quiet("");
        }
        self.clear_tool_sources_loading_activity();
        if matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_mcp_inventory()
        ) {
            let _ = self.execute_mcp_command();
        }
    }

    pub(super) fn execute_mcp_command(&mut self) -> anyhow::Result<()> {
        match self.info.session.daemon.request_mcp() {
            Ok(()) => {
                self.input_ui
                    .set_composer(ComposerMode::Picker(mcp_picker::loading()));
                self.set_status("loading session MCP inventory");
            }
            Err(error) => self.insert_entry(&super::Entry::Error(format!(
                "could not read session MCP inventory: {error}"
            ))),
        }
        Ok(())
    }
}
