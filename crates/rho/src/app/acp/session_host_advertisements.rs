//! Session menus are complete replacements, sent only when their contents change.

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, ConfigOptionUpdate, SessionConfigOption,
    SessionNotification, SessionUpdate,
};

use super::{commands, send_notification, AcpClientPort, Config, SessionHost};

/// The last payloads advertised to this host. Refresh caches advance only after
/// successful delivery. New/load seed them under the publication guard, which
/// clears them if the initial response or commands cannot be enqueued.
#[derive(Default)]
pub(super) struct Advertisements {
    config_options: Option<Vec<SessionConfigOption>>,
    commands: Option<Vec<AvailableCommand>>,
}

impl SessionHost {
    pub(in crate::app::acp) fn forget_advertisements(&mut self) {
        self.advertisements = Advertisements::default();
    }

    /// Seeds the menus returned by new/load: config options in the response,
    /// then commands after it, once the client knows the session.
    pub(in crate::app::acp) fn available_commands(
        &mut self,
        process_config: &Config,
    ) -> SessionNotification {
        let commands = self.discovered_commands(process_config);
        self.advertisements.config_options = Some(self.config_options(process_config));
        self.advertisements.commands = Some(commands.clone());
        SessionNotification::new(
            self.acp_session_id.clone(),
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands)),
        )
    }

    /// Refresh at prompt boundaries (including errors/cancellation), after an
    /// agent rebind, and after settings requests, while the host lock is held.
    /// Discovery here also notices files installed by tools or between prompts.
    pub(in crate::app::acp) async fn refresh_advertisements(
        &mut self,
        process_config: &Config,
        client: &dyn AcpClientPort,
    ) {
        let options = self.config_options(process_config);
        if self.advertisements.config_options.as_ref() != Some(&options) {
            let notification = SessionNotification::new(
                self.acp_session_id.clone(),
                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options.clone())),
            );
            match send_notification(client, notification).await {
                Ok(()) => self.advertisements.config_options = Some(options),
                Err(error) => tracing::warn!(%error, "could not send ACP config options"),
            }
        }
        let commands = self.discovered_commands(process_config);
        if self.advertisements.commands.as_ref() != Some(&commands) {
            let notification = SessionNotification::new(
                self.acp_session_id.clone(),
                SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
                    commands.clone(),
                )),
            );
            match send_notification(client, notification).await {
                Ok(()) => self.advertisements.commands = Some(commands),
                Err(error) => tracing::warn!(%error, "could not send ACP available commands"),
            }
        }
    }

    fn discovered_commands(&self, process_config: &Config) -> Vec<AvailableCommand> {
        let home = crate::paths::home_dir();
        commands::available_commands(&self.command_scope(home.as_deref(), process_config))
    }
}
