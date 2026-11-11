//! The session's MCP servers once the daemon restarted in place: the
//! restarted daemon connects them again, and its prompts wait for them
//! again, so the TUI reads whether they connect when it attaches there
//! and watches them until they have.

use std::{sync::Arc, time::Duration};

use rho_protocol::SessionId;

use super::ToolSources;
use crate::app::daemon_client::Link;

/// How late a held prompt can start after the servers connected again: the
/// interval the first connect is checked at (`TOOL_SOURCES_CHECK` in
/// `daemon_requests`). Each check is one read of an inventory the daemon
/// keeps in memory, and checks run only while a server connects.
const CHECK: Duration = Duration::from_millis(100);

/// The watch of a restarted daemon's session while its servers connect.
pub(super) struct Reconnecting(tokio::task::JoinHandle<()>);

impl Reconnecting {
    /// Checks the inventory of `session`, attached on `link`, until no
    /// server connects. One that cannot be read counts as connected, so no
    /// prompt waits on it forever.
    pub(super) fn watch(link: Arc<Link>, session: SessionId) -> Self {
        Self(tokio::spawn(async move {
            loop {
                tokio::time::sleep(CHECK).await;
                match ToolSources::read(&link, &session).await {
                    ToolSources::Loading => {}
                    ToolSources::Loaded => break,
                }
            }
        }))
    }
}

impl Drop for Reconnecting {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Resolves once the servers `reconnecting` watches connected, and ends
/// the watch; never while there is none. Cancel-safe.
pub(super) async fn connected(reconnecting: &mut Option<Reconnecting>) {
    match reconnecting {
        // A watch that died counts as connected, as an unread inventory does.
        Some(watch) => {
            let _ = (&mut watch.0).await;
        }
        None => std::future::pending().await,
    }
    *reconnecting = None;
}
