//! Attaching a session again on a new connection once the daemon
//! restarted: what the reattach asks for and what it comes back with.

use std::{future::Future, pin::Pin, sync::Arc};

use rho_protocol::{
    methods::{SessionAttach, SessionNew},
    ClientEnvironment, ConfigChangedParams, ErrorCode, SessionAttachParams, SessionAttachResponse,
    SessionId, SessionNewParams, SessionOptions,
};
use tokio::sync::mpsc;

use super::{
    initial_settings::{carry, changes_between, ModelChange},
    route,
    tool_sources::ToolSources,
    DaemonSettings,
};
use crate::{
    app::daemon_client::{Daemon, Link, Routed},
    tui::daemon_settings_read::{load_settings, route_config},
};

pub(super) type Reattach = Pin<Box<dyn Future<Output = anyhow::Result<Reattached>> + Send>>;

/// What a reattach came back with.
pub(super) struct Reattached {
    pub(super) response: SessionAttachResponse,
    /// After the daemon restarted: where the session attached again.
    pub(super) relinked: Option<Relinked>,
}

/// The session's attachment after the daemon restarted.
pub(super) struct Relinked {
    /// The connection it attached on.
    pub(super) link: Arc<Link>,
    /// The route its events now arrive on.
    pub(super) routed: mpsc::UnboundedReceiver<Routed>,
    /// The session's id: a new one when the old one had saved nothing.
    pub(super) session: SessionId,
    /// The settings, read again: the restarted daemon knows neither
    /// this connection's subscription nor the TUI's registration.
    pub(super) config_changes: mpsc::UnboundedReceiver<ConfigChangedParams>,
    pub(super) settings: anyhow::Result<DaemonSettings>,
    /// Whether the session's MCP servers connect, read again: the
    /// restarted daemon starts them again.
    pub(super) tool_sources: ToolSources,
}

/// Attaching a session again once the daemon restarted.
pub(super) struct Relink {
    pub(super) daemon: Arc<Daemon>,
    /// The attach, after the session's cursor.
    pub(super) params: SessionAttachParams,
    /// What opened the session, when it had saved nothing yet, and
    /// the options it ran with, which a new session in its place takes.
    pub(super) opened: Option<(SessionNewParams, Option<SessionOptions>)>,
}

impl Relink {
    /// Waits for `old`, the connection the daemon detached the session
    /// on, to close, then attaches on the next one, which waits for the
    /// restarted daemon. Once the router handed the session's old route
    /// `Lost`, a new route belongs to the next connection.
    pub(super) async fn run(self, old: Option<Arc<Link>>) -> anyhow::Result<Reattached> {
        let Self {
            daemon,
            params,
            opened,
        } = self;
        if let Some(old) = old {
            old.closed().await;
        }
        let routed = route(&daemon, &params.session);
        let config_changes = route_config(&daemon);
        // A daemon that failed to come back resets the connections that
        // waited for it; the second try starts a new daemon.
        let link = match daemon.link().await {
            Ok(link) => link,
            Err(_) => daemon.link().await?,
        };
        let settings = load_settings(&link).await;
        match link.request::<SessionAttach>(&params).await {
            Ok(response) => Ok(Reattached {
                response,
                relinked: Some(Relinked {
                    tool_sources: ToolSources::read(&link, &params.session).await,
                    link,
                    routed,
                    session: params.session,
                    config_changes,
                    settings,
                }),
            }),
            Err(error) => match opened {
                Some((mut opened, carried))
                    if error.error_code() == Some(ErrorCode::SessionNotFound) =>
                {
                    daemon.unroute(&params.session);
                    opened.environment = ClientEnvironment::of_this_process();
                    let created = link.request::<SessionNew>(&opened).await?;
                    let session = created.session;
                    // The settings the person changed carry over; one the
                    // restarted daemon refuses keeps its default.
                    let changes = carried
                        .map(|carried| {
                            changes_between(&created.options, &carried, ModelChange::Setup)
                        })
                        .unwrap_or_default();
                    carry(&link, &session, changes).await;
                    let routed = route(&daemon, &session);
                    let attach = SessionAttachParams {
                        session: session.clone(),
                        after: None,
                        ..params
                    };
                    let response = link.request::<SessionAttach>(&attach).await?;
                    Ok(Reattached {
                        response,
                        relinked: Some(Relinked {
                            tool_sources: ToolSources::read(&link, &session).await,
                            link,
                            routed,
                            session,
                            config_changes,
                            settings,
                        }),
                    })
                }
                _ => Err(error.into()),
            },
        }
    }
}
