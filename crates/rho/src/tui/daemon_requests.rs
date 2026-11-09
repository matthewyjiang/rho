//! Serial configuration and prompt requests on the attached connection.
//!
//! Configure can wait for an active turn (including an approval) to end. Its
//! response must never own the TUI event loop. Prompts share the queue so a
//! later prompt cannot overtake a configuration change. Cancel and answers
//! deliberately bypass this queue, since they may unblock that change. MCP
//! inventory reads use an independent worker to stay usable during the turn.

use std::sync::Arc;

use rho_protocol::{
    methods::{SessionConfigure, SessionPrompt},
    ClientEnvironment, ContentBlock, SessionChange, SessionConfigureParams, SessionId,
    SessionPromptParams,
};
use tokio::sync::{mpsc, oneshot};

use super::{daemon_events::DaemonView, daemon_options::setting_name};
use crate::app::daemon_client::Link;

enum Request {
    Configure(SessionChange),
    Prompt {
        content: Vec<ContentBlock>,
        result: oneshot::Sender<anyhow::Result<()>>,
    },
}

pub(super) struct DaemonRequests {
    sender: mpsc::UnboundedSender<Request>,
    pub(super) views: mpsc::UnboundedReceiver<DaemonView>,
    task: tokio::task::JoinHandle<()>,
    inventory: mpsc::UnboundedSender<()>,
    inventory_task: tokio::task::JoinHandle<()>,
}

impl DaemonRequests {
    /// `link` must come from `DaemonSession::attached_link`, never reconnect.
    pub(super) fn new(link: Arc<Link>, session: SessionId) -> Self {
        let (sender, mut requests) = mpsc::unbounded_channel();
        let (views, receiver) = mpsc::unbounded_channel();
        let (inventory, mut inventory_requests) = mpsc::unbounded_channel();
        let inventory_link = Arc::clone(&link);
        let inventory_session = session.clone();
        let inventory_views = views.clone();
        // Inventory reads bypass configure, which can wait for a turn to end.
        let inventory_task = tokio::spawn(async move {
            while inventory_requests.recv().await.is_some() {
                let result = inventory_link
                    .request::<rho_protocol::methods::SessionMcp>(&rho_protocol::SessionMcpParams {
                        session: inventory_session.clone(),
                    })
                    .await
                    .map(Box::new)
                    .map_err(|error| format!("could not read session MCP inventory: {error}"));
                let _ = inventory_views.send(DaemonView::McpInventory(result));
            }
        });
        let task = tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                match request {
                    Request::Configure(change) => {
                        let setting = setting_name(&change);
                        let response = link
                            .request::<SessionConfigure>(&SessionConfigureParams {
                                session: session.clone(),
                                change,
                            })
                            .await;
                        link.fence().await;
                        let _ = views.send(DaemonView::ConfigureFinished);
                        if let Err(error) = response {
                            let _ = views.send(DaemonView::Error(format!(
                                "could not change the {setting}: {error}"
                            )));
                        }
                        // OptionsChanged, not an optimistic local update, is
                        // authoritative. The response describes the same change.
                    }
                    Request::Prompt { content, result } => {
                        let response = link
                            .request::<SessionPrompt>(&SessionPromptParams {
                                session: session.clone(),
                                content,
                                timeout_ms: None,
                                environment: ClientEnvironment::of_this_process(),
                            })
                            .await
                            .map(|_| ())
                            .map_err(anyhow::Error::new);
                        link.fence().await;
                        let _ = result.send(response);
                    }
                }
            }
        });
        Self {
            sender,
            views: receiver,
            task,
            inventory,
            inventory_task,
        }
    }

    pub(super) fn request_mcp(&self) -> anyhow::Result<()> {
        self.inventory
            .send(())
            .map_err(|_| anyhow::anyhow!("lost the connection to the rho daemon"))
    }

    pub(super) fn configure(&self, change: SessionChange) -> anyhow::Result<()> {
        self.sender
            .send(Request::Configure(change))
            .map_err(|_| anyhow::anyhow!("lost the connection to the rho daemon"))
    }

    pub(super) async fn prompt(&self, content: Vec<ContentBlock>) -> anyhow::Result<()> {
        let (result, response) = oneshot::channel();
        self.sender
            .send(Request::Prompt { content, result })
            .map_err(|_| anyhow::anyhow!("lost the connection to the rho daemon"))?;
        response.await?
    }
}

impl Drop for DaemonRequests {
    fn drop(&mut self) {
        self.task.abort();
        self.inventory_task.abort();
    }
}
