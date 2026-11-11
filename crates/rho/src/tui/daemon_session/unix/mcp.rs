//! What the session's MCP servers offer the composer. The daemon's session
//! connected them, so the TUI reads their prompts and resources, one
//! resource's bodies, and the values one suggests for a prompt argument
//! from the daemon, each without waiting for the session's turn.

use rho_protocol::{
    methods::{SessionMcpCatalogRead, SessionMcpComplete, SessionMcpResourceRead},
    ClientError, SessionMcpCatalogParams, SessionMcpCompleteParams, SessionMcpResourceContent,
    SessionMcpResourceParams,
};
use tokio::sync::watch;

use super::{DaemonSession, Target, LOST};
use crate::{
    tools::mcp::{
        catalog::{McpPrompt, McpPromptArgument},
        McpCompletionSupport, McpResource, McpResourceContent,
    },
    tui::{daemon_requests::on_attachment, mcp_source::McpOffers},
};

/// Reads from the session's MCP servers, for work that outlives a borrow
/// of the session. A read waits out a daemon restart, as the session's
/// other requests do, and fails once following ended.
#[derive(Clone)]
pub(in crate::tui) struct DaemonMcp {
    target: watch::Receiver<Target>,
}

impl DaemonSession {
    pub(in crate::tui) fn mcp(&self) -> DaemonMcp {
        DaemonMcp {
            target: self.target.subscribe(),
        }
    }
}

impl DaemonMcp {
    /// What the servers offer now; `None` when the daemon did not say.
    pub(in crate::tui) async fn offers(mut self) -> Option<McpOffers> {
        let (_, answer) = on_attachment(&mut self.target, |link, session| async move {
            link.request::<SessionMcpCatalogRead>(&SessionMcpCatalogParams { session })
                .await
        })
        .await?;
        let catalog = answer.ok()?;
        Some(McpOffers {
            prompts: catalog
                .prompts
                .into_iter()
                .map(|prompt| {
                    let support = if prompt.completes_arguments {
                        McpCompletionSupport::Declared
                    } else {
                        McpCompletionSupport::Absent
                    };
                    let prompt = McpPrompt {
                        server: prompt.server,
                        name: prompt.name,
                        title: prompt.title,
                        description: prompt.description,
                        arguments: prompt
                            .arguments
                            .into_iter()
                            .map(|argument| McpPromptArgument {
                                name: argument.name,
                                description: argument.description,
                                required: argument.required,
                            })
                            .collect(),
                    };
                    (prompt, support)
                })
                .collect(),
            resources: catalog
                .resources
                .into_iter()
                .map(|resource| McpResource {
                    server: resource.server,
                    uri: resource.uri,
                    name: resource.name,
                    title: resource.title,
                    description: resource.description,
                    mime_type: resource.mime_type,
                    templated: resource.templated,
                })
                .collect(),
        })
    }

    /// One resource's bodies, or why the read failed, in the daemon's words.
    /// A text body longer than `max_text_bytes` may arrive cut short, still
    /// longer than that.
    pub(in crate::tui) async fn read_resource(
        mut self,
        server: String,
        uri: String,
        max_text_bytes: usize,
    ) -> Result<Vec<McpResourceContent>, String> {
        let answer = on_attachment(&mut self.target, |link, session| {
            let params = SessionMcpResourceParams {
                session,
                server: server.clone(),
                uri: uri.clone(),
                max_text_bytes: Some(max_text_bytes as u64),
            };
            async move { link.request::<SessionMcpResourceRead>(&params).await }
        })
        .await;
        match answer {
            Some((_, Ok(response))) => Ok(response
                .contents
                .into_iter()
                .map(|content| match content {
                    SessionMcpResourceContent::Text {
                        uri,
                        mime_type,
                        text,
                    } => McpResourceContent::Text {
                        uri,
                        mime_type,
                        text,
                    },
                    SessionMcpResourceContent::Blob {
                        uri,
                        mime_type,
                        blob,
                    } => McpResourceContent::Blob {
                        uri,
                        mime_type,
                        blob,
                    },
                    // `Unknown`, or a kind a newer daemon sends.
                    _ => McpResourceContent::Unsupported,
                })
                .collect()),
            Some((_, Err(ClientError::Rpc { error, .. }))) => Err(error.message),
            Some((_, Err(error))) => Err(error.to_string()),
            None => Err(LOST.to_owned()),
        }
    }

    /// Values the server suggests for one prompt argument; none when it has
    /// none to give or could not be asked, since a suggestion is only help.
    pub(in crate::tui) async fn complete_prompt_argument(
        mut self,
        server: String,
        prompt: String,
        argument: String,
        typed: String,
    ) -> Vec<String> {
        let answer = on_attachment(&mut self.target, |link, session| {
            let params = SessionMcpCompleteParams {
                session,
                server: server.clone(),
                prompt: prompt.clone(),
                argument: argument.clone(),
                value: typed.clone(),
            };
            async move { link.request::<SessionMcpComplete>(&params).await }
        })
        .await;
        match answer {
            Some((_, Ok(response))) => response.values,
            Some((_, Err(_))) | None => Vec::new(),
        }
    }
}
