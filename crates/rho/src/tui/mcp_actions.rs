//! The `/mcp` command: show configured servers and session load status.

use super::{mcp_picker, App, ComposerMode, InteractiveRuntime, StatusSource};

impl App {
    pub(super) fn show_daemon_mcp(
        &mut self,
        result: Result<Box<rho_protocol::SessionMcpInventory>, String>,
    ) {
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

    /// Whether the session's tool sources still load, so a prompt waits:
    /// the daemon session's when the TUI follows one, else the runtime's.
    pub(super) fn tool_sources_pending(&self, agent: &InteractiveRuntime) -> bool {
        match &self.info.session.daemon {
            Some(session) => session.tool_sources_loading(),
            None => agent.tool_sources_pending(),
        }
    }

    /// The daemon session's MCP servers stopped connecting: as when the
    /// in-process load lands, held prompts may go and an open `/mcp`
    /// shows where the servers ended up.
    pub(super) fn show_daemon_tool_sources_loaded(&mut self) {
        if let Some(session) = self.info.session.daemon.as_mut() {
            session.tool_sources_loaded();
        }
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
        if let Some(session) = &self.info.session.daemon {
            match session.request_mcp() {
                Ok(()) => {
                    self.input_ui
                        .set_composer(ComposerMode::Picker(mcp_picker::loading()));
                    self.set_status("loading session MCP inventory");
                }
                Err(error) => self.insert_entry(&super::Entry::Error(format!(
                    "could not read session MCP inventory: {error}"
                ))),
            }
            return Ok(());
        }
        let config_path = self.info.services.config_repository.configured_path()?;
        let inventory =
            crate::app::session_mcp_inventory(&self.mcp_report, &self.mcp_catalog, &config_path);
        let picker = mcp_picker::picker(&inventory);
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status("mcp servers");
        Ok(())
    }
}
