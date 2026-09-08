//! Explicit desktop access, kept separate from ordinary MCP configuration.

use crate::tools::computer_use::{ComputerUseSession, ComputerUseStatus};

use super::{App, CommandInvocation, Entry, InteractiveRuntime};

impl App {
    pub(super) async fn execute_computer_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.computer_use = agent.computer_use().cloned();
        match invocation.args.trim() {
            "on" => {
                if self.computer_connect_pending() {
                    self.set_status("computer use is connecting; /computer off cancels");
                    return Ok(());
                }
                let session = match agent.authorize_computer_use() {
                    Ok(session) => session,
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not enable computer use: {error}"
                        )));
                        return Ok(());
                    }
                };
                if session.status() == ComputerUseStatus::Connected {
                    self.show_computer_status();
                    return Ok(());
                }
                self.insert_entry(&Entry::Notice(
                    "computer use enabled for this session: Cua Driver can access the local desktop, including signed-in apps. This grants computer actions without per-action Rho approval, including supervised mode. Captured images are sent to your model provider and saved in session history. /computer off revokes access".into(),
                ));
                session.start_connect();
                self.set_status(
                    "connecting computer use through Cua Driver; /computer off cancels",
                );
            }
            "off" | "stop" => {
                self.stop_computer_connection().await;
                if let Err(error) = agent.disable_computer_use().await {
                    self.insert_entry(&Entry::Error(format!(
                        "could not refresh computer tool registration: {error}"
                    )));
                }
                self.show_computer_off();
            }
            _ => self.show_computer_command(&invocation),
        }
        Ok(())
    }

    pub(super) async fn execute_computer_command_during_turn(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        match invocation.args.trim() {
            "off" | "stop" => {
                // Revoke the shared handle immediately. The old registered tool
                // fails closed; the runtime can refresh its registry when idle.
                self.stop_computer_connection().await;
                self.show_computer_off();
            }
            "on" => self.set_status("interrupt the current turn before enabling computer use"),
            _ => self.show_computer_command(&invocation),
        }
        Ok(())
    }

    fn show_computer_command(&mut self, invocation: &CommandInvocation) {
        match invocation.args.trim() {
            "" | "status" => self.show_computer_status(),
            "setup" => {
                self.insert_entry(&Entry::Notice(ComputerUseSession::setup_guidance().into()))
            }
            _ => self.insert_entry(&Entry::Error(
                "usage: /computer [status|setup|on|off]".into(),
            )),
        }
    }

    fn show_computer_status(&mut self) {
        let status = match self.computer_use.as_ref().map(ComputerUseSession::status) {
            None | Some(ComputerUseStatus::Off) => "off",
            Some(ComputerUseStatus::Connecting) => "connecting",
            Some(ComputerUseStatus::Connected) => "connected",
        };
        let driver = self
            .computer_use
            .as_ref()
            .and_then(ComputerUseSession::driver_path)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not detected; /computer setup for instructions".into());
        self.insert_entry(&Entry::Notice(format!(
            "Computer use, powered by Cua Driver\nStatus: {status}\nDriver: {driver}\nConnection does not verify desktop permissions. /computer on grants this session access to the local desktop; /computer off revokes it"
        )));
    }

    fn show_computer_off(&mut self) {
        self.insert_entry(&Entry::Notice("computer use off; this session is disconnected. Completed desktop actions cannot be undone by disconnecting".into()));
        self.set_status("computer use off");
    }

    pub(super) async fn stop_computer_connection(&mut self) {
        if let Some(session) = &self.computer_use {
            session.disconnect().await;
        }
    }

    pub(super) fn computer_connect_pending(&self) -> bool {
        self.computer_use
            .as_ref()
            .is_some_and(ComputerUseSession::connection_pending)
    }

    pub(super) async fn poll_computer_connection(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> bool {
        if agent.is_session_busy() {
            return false;
        }
        let Some(session) = &self.computer_use else {
            return false;
        };
        let Some(result) = session.take_connect_result().await else {
            return false;
        };
        let result = match result {
            Ok(()) => agent.enable_connected_computer_use().await,
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => {
                self.insert_entry(&Entry::Notice("computer use connected through Cua Driver; the computer tool is available for the next turn".into()));
                self.set_status("computer use connected");
            }
            Err(error) => {
                self.stop_computer_connection().await;
                self.insert_entry(&Entry::Error(format!(
                    "could not connect computer use: {error}"
                )));
                self.set_status("computer use off; /computer setup for help");
            }
        }
        true
    }
}
