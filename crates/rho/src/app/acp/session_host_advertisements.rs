//! Session menus are complete replacements, sent only when their contents change.

use std::future::Future;

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, ConfigOptionUpdate, SessionConfigOption, SessionId,
    SessionNotification, SessionUpdate,
};

use super::{
    commands, runtime_config_options, send_notification, AcpClientPort, Config, SessionHost,
    SessionRuntime, Toggles,
};
use crate::app::session_commands::CommandScope;

/// The last payloads advertised to this host. Refresh caches advance only after
/// successful delivery. New/load seed them under the publication guard, which
/// clears them if the initial response or commands cannot be enqueued.
#[derive(Default)]
pub(super) struct Advertisements {
    config_options: Option<Vec<SessionConfigOption>>,
    commands: Option<Vec<AvailableCommand>>,
}

impl Advertisements {
    /// Sends session `session_id`'s menus, as `runtime` has them now, where
    /// they differ from the last ones advertised. Both are read before the
    /// first send, so the refresh holds nothing of the runtime.
    ///
    /// Refresh at prompt boundaries (including errors/cancellation), after an
    /// agent rebind or a plan's mode switch, and after settings requests,
    /// while the host lock is held. Discovery here also notices files
    /// installed by tools or between prompts.
    pub(super) fn refresh<'a>(
        &'a mut self,
        session_id: &'a SessionId,
        runtime: &SessionRuntime,
        toggles: Toggles,
        process_config: &Config,
        client: &'a dyn AcpClientPort,
    ) -> impl Future<Output = ()> + Send + 'a {
        let options = runtime_config_options(runtime, process_config, toggles);
        let commands = discovered_commands(runtime, process_config);
        async move {
            if self.config_options.as_ref() != Some(&options) {
                let notification = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options.clone())),
                );
                match send_notification(client, notification).await {
                    Ok(()) => self.config_options = Some(options),
                    Err(error) => tracing::warn!(%error, "could not send ACP config options"),
                }
            }
            if self.commands.as_ref() != Some(&commands) {
                let notification = SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
                        commands.clone(),
                    )),
                );
                match send_notification(client, notification).await {
                    Ok(()) => self.commands = Some(commands),
                    Err(error) => tracing::warn!(%error, "could not send ACP available commands"),
                }
            }
        }
    }
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
        let commands = discovered_commands(&self.runtime, process_config);
        self.advertisements.config_options = Some(self.config_options(process_config));
        self.advertisements.commands = Some(commands.clone());
        SessionNotification::new(
            self.acp_session_id.clone(),
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands)),
        )
    }

    /// Refreshes this session's menus; see [`Advertisements::refresh`].
    pub(in crate::app::acp) async fn refresh_advertisements(
        &mut self,
        process_config: &Config,
        client: &dyn AcpClientPort,
    ) {
        self.advertisements
            .refresh(
                &self.acp_session_id,
                &self.runtime,
                self.host_features.toggles,
                process_config,
                client,
            )
            .await;
    }
}

fn discovered_commands(runtime: &SessionRuntime, process_config: &Config) -> Vec<AvailableCommand> {
    let home = crate::paths::home_dir();
    commands::available_commands(&CommandScope::of(runtime, home.as_deref(), process_config))
}
