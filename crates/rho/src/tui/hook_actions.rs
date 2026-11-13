//! The `/hooks` command: reload hook configuration and show the spawn contract.

use super::App;

impl App {
    pub(super) fn execute_hooks_command(&mut self) -> anyhow::Result<()> {
        self.request_daemon_report(rho_protocol::SessionReport::Hooks);
        Ok(())
    }
}
