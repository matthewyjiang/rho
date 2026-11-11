//! Where the composer's MCP prompts and resources come from: the servers
//! the session's tools use. An in-process session connected them itself,
//! so the TUI reads its catalog. A daemon session's are the daemon's, so
//! the TUI reads them from there (`daemon_session`'s `DaemonMcp`).
//!
//! Palette matching is synchronous, so what a daemon session's servers
//! offer is read ahead into [`DaemonMcpOffers`] and matched from there. The
//! read is clock-free, like `mcp_argument_completion`'s: one starts when
//! the composer begins an `@` mention or an MCP prompt command, and one
//! more once the session's tool sources loaded, if the first ran before
//! they had.

use super::{daemon_session::DaemonMcp, file_picker::PathTokenSource, App, ComposerMode};
use crate::{
    commands,
    tools::mcp::{
        catalog::McpPrompt, prompt_command, McpCatalog, McpCompletionSupport, McpResource,
        McpResourceContent,
    },
};

/// The prompts and resources a session's MCP servers offer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct McpOffers {
    /// Each prompt, with whether its server suggests values for its
    /// arguments.
    pub(super) prompts: Vec<(McpPrompt, McpCompletionSupport)>,
    pub(super) resources: Vec<McpResource>,
}

/// What a daemon session's servers offer, as last read.
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

/// A handle that fetches from the session's MCP servers, for a task that
/// outlives a borrow of the app.
#[derive(Clone)]
pub(super) enum McpSource {
    InProcess(McpCatalog),
    Daemon(DaemonMcp),
}

impl McpSource {
    /// One resource's bodies, or why the read failed. The caller uses at
    /// most `max_text_bytes` of each text body, so a daemon need not send
    /// the rest of a longer one.
    pub(super) async fn read_resource(
        self,
        server: String,
        uri: String,
        max_text_bytes: usize,
    ) -> Result<Vec<McpResourceContent>, String> {
        match self {
            Self::InProcess(catalog) => catalog
                .read_resource(&server, &uri)
                .await
                .map_err(|error| error.to_string()),
            Self::Daemon(mcp) => mcp.read_resource(server, uri, max_text_bytes).await,
        }
    }

    /// Values the server suggests for one prompt argument, or none.
    pub(super) async fn complete_prompt_argument(
        self,
        server: String,
        prompt: String,
        argument: String,
        typed: String,
    ) -> Vec<String> {
        match self {
            Self::InProcess(catalog) => {
                catalog
                    .complete_prompt_argument(&server, &prompt, &argument, &typed)
                    .await
            }
            Self::Daemon(mcp) => {
                mcp.complete_prompt_argument(server, prompt, argument, typed)
                    .await
            }
        }
    }
}

impl App {
    pub(super) fn mcp_source(&self) -> McpSource {
        match &self.info.session.daemon {
            Some(session) => McpSource::Daemon(session.mcp()),
            None => McpSource::InProcess(self.mcp_catalog.clone()),
        }
    }

    /// Every resource the session's servers offer, for `@` matching.
    pub(super) fn mcp_resources(&self) -> Vec<McpResource> {
        if self.info.session.daemon.is_some() {
            self.daemon_mcp_offers.offers.resources.clone()
        } else if self.mcp_catalog.is_empty() {
            Vec::new()
        } else {
            self.mcp_catalog.resources()
        }
    }

    /// The prompt `server` offers as `name`, and whether that server
    /// suggests values for its arguments.
    pub(super) fn mcp_prompt(
        &self,
        server: &str,
        name: &str,
    ) -> Option<(McpPrompt, McpCompletionSupport)> {
        if self.info.session.daemon.is_some() {
            return self
                .daemon_mcp_offers
                .offers
                .prompts
                .iter()
                .find(|(prompt, _)| prompt.server == server && prompt.name == name)
                .cloned();
        }
        let prompt = self
            .mcp_catalog
            .prompts()
            .into_iter()
            .find(|prompt| prompt.server == server && prompt.name == name)?;
        Some((prompt, self.mcp_catalog.completion_support(server)))
    }

    /// Forget what the daemon session left behind offered.
    pub(super) fn reset_daemon_mcp_offers(&mut self) {
        self.daemon_mcp_offers = DaemonMcpOffers::default();
    }

    /// Collect a finished read of what a daemon session's servers offer and
    /// start the next one the composer calls for. Returns whether the
    /// palettes must redraw.
    pub(super) async fn poll_daemon_mcp_offers(&mut self) -> bool {
        let Some(session) = &self.info.session.daemon else {
            return false;
        };
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
