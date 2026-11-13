//! Whether the session's tool sources still load. The daemon loads its MCP
//! servers and OpenAPI sources after the session opens, and again once it
//! restarted in place, and its prompts wait for them. So the TUI reads whether
//! they load each time the session attaches, holds a prompt meanwhile, and
//! watches them until they have.

use std::{sync::Arc, time::Duration};

use rho_protocol::{
    methods::SessionMcp, SessionId, SessionMcpInventory, SessionMcpParams, SessionMcpStatus,
};

use crate::app::daemon_client::Link;

/// Whether the session's tool sources still load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum ToolSources {
    Loading,
    Loaded,
}

impl ToolSources {
    /// A daemon that does not say whether its tool sources load still
    /// says which servers connect.
    fn of(inventory: &SessionMcpInventory) -> Self {
        if inventory.tool_sources_loading
            || inventory
                .servers
                .iter()
                .any(|server| server.status == SessionMcpStatus::Connecting)
        {
            Self::Loading
        } else {
            Self::Loaded
        }
    }

    /// Reads the inventory of `session`, attached on `link`. One that
    /// cannot be read counts as loaded, so no prompt waits on it forever.
    pub(super) async fn read(link: &Link, session: &SessionId) -> Self {
        link.request::<SessionMcp>(&SessionMcpParams {
            session: session.clone(),
        })
        .await
        .map_or(Self::Loaded, |inventory| Self::of(&inventory))
    }
}

/// How late a held prompt can start after the session's tool sources
/// loaded. Each check is one read of an inventory the daemon keeps in
/// memory, and checks run only while they load.
const CHECK: Duration = Duration::from_millis(100);

/// The one watch of a session's tool sources while they load, on the
/// connection the session is attached on. The session drops it when that
/// connection goes and starts another once it attached again, so a watch
/// never reports for a daemon the session has left.
pub(super) struct Watch(tokio::task::JoinHandle<()>);

impl Watch {
    /// The watch `tool_sources` calls for, as read for `session` on
    /// `link`: none once they loaded.
    pub(super) fn of(
        tool_sources: ToolSources,
        link: &Arc<Link>,
        session: &SessionId,
    ) -> Option<Self> {
        match tool_sources {
            ToolSources::Loading => {
                let link = Arc::clone(link);
                let session = session.clone();
                Some(Self(tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(CHECK).await;
                        match ToolSources::read(&link, &session).await {
                            ToolSources::Loading => {}
                            ToolSources::Loaded => break,
                        }
                    }
                })))
            }
            ToolSources::Loaded => None,
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Resolves once the tool sources `watch` follows loaded, and ends the
/// watch; never while there is none. Cancel-safe.
pub(super) async fn loaded(watch: &mut Option<Watch>) {
    match watch {
        // A watch that died counts as loaded, as an unread inventory does.
        Some(watch) => {
            let _ = (&mut watch.0).await;
        }
        None => std::future::pending().await,
    }
    *watch = None;
}
