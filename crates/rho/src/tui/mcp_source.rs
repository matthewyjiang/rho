//! Where the composer's MCP prompts and resources come from: the servers
//! the session's tools use, which are the daemon's. The TUI reads them from
//! there (`daemon_session`'s `DaemonMcp`).
//!
//! Palette matching is synchronous, so what the session's servers offer is
//! read ahead into [`DaemonMcpOffers`] and matched from there. The read is
//! clock-free, like `mcp_argument_completion`'s: one starts when the
//! composer begins an `@` mention or an MCP prompt command, and one more
//! once the session's tool sources loaded, if the first ran before they had.

use super::{daemon_session::DaemonMcp, file_picker::PathTokenSource, App, ComposerMode};
use crate::{
    commands,
    tools::mcp::{catalog::McpPrompt, prompt_command, McpCompletionSupport, McpResource},
};

/// The prompts and resources a session's MCP servers offer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct McpOffers {
    /// Each prompt, with whether its server suggests values for its
    /// arguments.
    pub(super) prompts: Vec<(McpPrompt, McpCompletionSupport)>,
    pub(super) resources: Vec<McpResource>,
}

/// What the session's servers offer, as last read.
#[derive(Debug, Default)]
pub(super) struct DaemonMcpOffers {
    offers: McpOffers,
    /// The one read in flight; `None` from it when the daemon did not say.
    pending: Option<tokio::task::JoinHandle<Option<McpOffers>>>,
    /// The read the composer's current mention or command started; `None`
    /// while the composer shows neither.
    asked: Option<Asked>,
}

/// Whether the session's tool sources still loaded when a read started,
/// which says whether the servers may offer more once they have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    WhileLoading,
    Loaded,
}

impl DaemonMcpOffers {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

impl App {
    /// A handle that fetches from the session's MCP servers, for a task
    /// that outlives a borrow of the app.
    pub(super) fn mcp_source(&self) -> DaemonMcp {
        self.info.session.daemon.mcp()
    }

    /// Every resource the session's servers offer, for `@` matching.
    pub(super) fn mcp_resources(&self) -> Vec<McpResource> {
        self.daemon_mcp_offers.offers.resources.clone()
    }

    /// The prompt `server` offers as `name`, and whether that server
    /// suggests values for its arguments.
    pub(super) fn mcp_prompt(
        &self,
        server: &str,
        name: &str,
    ) -> Option<(McpPrompt, McpCompletionSupport)> {
        self.daemon_mcp_offers
            .offers
            .prompts
            .iter()
            .find(|(prompt, _)| prompt.server == server && prompt.name == name)
            .cloned()
    }

    /// Forget what the session left behind offered.
    pub(super) fn reset_daemon_mcp_offers(&mut self) {
        self.daemon_mcp_offers = DaemonMcpOffers::default();
    }

    /// Collect a finished read of what the session's servers offer and
    /// start the next one the composer calls for. Returns whether the
    /// palettes must redraw.
    pub(super) async fn poll_daemon_mcp_offers(&mut self) -> bool {
        let session = &self.info.session.daemon;
        let mut redraw = false;
        if let Some(pending) = self
            .daemon_mcp_offers
            .pending
            .take_if(|pending| pending.is_finished())
        {
            // A read the daemon did not answer keeps what was offered before.
            if let Ok(Some(offers)) = pending.await {
                if offers != self.daemon_mcp_offers.offers {
                    self.daemon_mcp_offers.offers = offers;
                    // Matches cached for the mention being typed predate it.
                    self.palette_caches.clear_file();
                    redraw = true;
                }
            }
        }

        let text = self.input_ui.text();
        let reads_mcp = matches!(self.input_ui.composer(), ComposerMode::Input)
            && (self
                .active_path_token()
                .is_some_and(|token| token.source == PathTokenSource::Mention)
                || commands::command_prefix(text)
                    .and_then(prompt_command::parse_command)
                    .is_some());
        if !reads_mcp {
            self.daemon_mcp_offers.asked = None;
            return redraw;
        }
        let loading = session.tool_sources_loading();
        let stale = match self.daemon_mcp_offers.asked {
            None => true,
            Some(Asked::WhileLoading) => !loading,
            Some(Asked::Loaded) => false,
        };
        if stale && self.daemon_mcp_offers.pending.is_none() {
            self.daemon_mcp_offers.asked = Some(if loading {
                Asked::WhileLoading
            } else {
                Asked::Loaded
            });
            self.daemon_mcp_offers.pending = Some(tokio::spawn(session.mcp().offers()));
        }
        redraw
    }
}
