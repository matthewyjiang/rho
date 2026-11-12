//! Connecting to the daemon and attaching to a session on that
//! connection, as the TUI starts to follow one.

use std::sync::Arc;

use anyhow::Context;
use rho_protocol::{
    methods::SessionAttach, ClientEnvironment, ConfigChangedParams, JobState, SessionAttachParams,
    SessionChange, SessionId, SessionNewParams,
};
use tokio::sync::mpsc;

use super::{
    begin_replay,
    initial_settings::{configure_all, Refused},
    route,
    tool_sources::{self, ToolSources},
    Attachment, DaemonSession, LOST,
};
use crate::{
    app::{
        daemon_client::{Daemon, Link, StreamPosition},
        SettingOrigin, SettingOrigins,
    },
    tui::{
        daemon_events::{DaemonEvents, DaemonView},
        daemon_launch::{DaemonLaunch, InitialSettings},
        daemon_options::setting_name,
        daemon_settings::DaemonSettings,
        daemon_settings_read::{load_settings, route_config},
    },
};

/// A daemon connection a session is followed on, and the settings read
/// on it.
pub(super) struct Connection {
    pub(super) daemon: Arc<Daemon>,
    pub(super) link: Arc<Link>,
    pub(super) config_changes: mpsc::UnboundedReceiver<ConfigChangedParams>,
    pub(super) settings: anyhow::Result<DaemonSettings>,
}

impl Connection {
    /// A connection of its own, which starts the daemon when none runs,
    /// with the settings read on it.
    pub(super) async fn open() -> anyhow::Result<Self> {
        let daemon = Arc::new(Daemon::for_frontend());
        let link = daemon
            .link()
            .await
            .context("could not reach the rho daemon")?;
        // Routed before the reads, so no change after them is dropped.
        let config_changes = route_config(&daemon);
        let settings = load_settings(&link).await;
        Ok(Self {
            daemon,
            link,
            config_changes,
            settings,
        })
    }
}

/// An attachment just made, and what it leaves to do.
pub(super) struct Attached {
    pub(super) attachment: Attachment,
    /// The connection it attached on.
    pub(super) link: Arc<Link>,
    /// The views its response makes.
    pub(super) views: Vec<DaemonView>,
    /// Launch settings that wait for the turn under way.
    pub(super) queued: Vec<SessionChange>,
}

impl DaemonSession {
    /// Attaches to session `id`, which `opened` opened unless it was
    /// saved, on `daemon`'s connection, and applies `settings`.
    pub(super) async fn attach(
        daemon: &Arc<Daemon>,
        id: SessionId,
        opened: Option<SessionNewParams>,
        launch: &DaemonLaunch,
        settings: InitialSettings,
    ) -> anyhow::Result<Attached> {
        // Routed before the attach, so the replay that follows its
        // response is not dropped.
        let routed = route(daemon, &id);
        let params = SessionAttachParams {
            session: id.clone(),
            after: None,
            environment: ClientEnvironment::of_this_process(),
            setup: launch.setup.clone(),
        };
        let mut response = match daemon.request::<SessionAttach>(&params).await {
            Ok(response) => response,
            Err(error) => {
                daemon.unroute(&id);
                return Err(anyhow::Error::new(error)
                    .context(format!("could not attach to daemon session {id}")));
            }
        };
        let link = daemon.current().await.context(LOST)?;
        // After the attach, which loads a saved session. The daemon
        // also sends each change's options on the session's stream.
        let at_rest = !matches!(
            response.state,
            JobState::Queued | JobState::Running | JobState::WaitingForInput
        );
        // A change waits for the turn under way, which may wait for
        // this client: those go through the request queue once the
        // session is followed, and a refusal shows as a row.
        let mut queued = Vec::new();
        // The launch's changes the daemon took now; queued ones count once
        // it takes them (`DaemonRequests::picks`).
        let mut origins = SettingOrigins::default();
        let required: Vec<SessionChange> = match settings {
            InitialSettings::Kept => Vec::new(),
            InitialSettings::Required(changes) => changes
                .into_iter()
                .map(|change| change.of(&response.options))
                .collect(),
        };
        if !at_rest {
            queued = required;
        } else {
            for change in &required {
                origins.set(change, SettingOrigin::Chosen);
            }
            match configure_all(&link, &id, required).await {
                // The session's options are what the changes left, not
                // what the attach answered before them: the chrome shows
                // them, and a `/new` sent before the stream's own report
                // of the changes is read carries them on.
                Ok(Some(changed)) => response.options = changed,
                Ok(None) => {}
                Err(refused) => {
                    let Refused { change, error } = *refused;
                    daemon.unroute(&id);
                    return Err(anyhow::Error::new(error).context(format!(
                        "could not set the session's {}",
                        setting_name(&change)
                    )));
                }
            }
        }
        let tool_sources = ToolSources::read(&link, &id).await;
        let tool_source_watch = tool_sources::Watch::of(tool_sources, &link, &id);
        let mut events = DaemonEvents::default();
        let views = begin_replay(&mut events, &response);
        let attachment = Attachment {
            position: StreamPosition::attached(&response),
            saved: opened.is_none() || response.title.is_some(),
            keeps_file: !opened.as_ref().is_some_and(|opened| {
                opened
                    .setup
                    .without
                    .contains(&rho_protocol::SessionPart::Saving)
            }),
            id,
            opened,
            workspace: response.workspace,
            routed,
            options: Some(response.options),
            origins,
            chosen_on: Arc::clone(&launch.config_file),
            events,
            ended: false,
            reattach: None,
            tool_sources,
            tool_source_watch,
        };
        Ok(Attached {
            attachment,
            link,
            views,
            queued,
        })
    }
}
