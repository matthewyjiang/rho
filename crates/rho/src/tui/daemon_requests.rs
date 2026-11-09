//! Serial configuration and prompt requests on the attached connection.
//!
//! Configure can wait for an active turn (including an approval) to end. Its
//! response must never own the TUI event loop. Prompts share the queue so a
//! later prompt cannot overtake a configuration change. Cancel and answers
//! deliberately bypass this queue, since they may unblock that change. MCP
//! inventory reads, goal actions, and settings writes use an independent
//! control worker to stay usable during the turn: a goal runs as one long
//! turn, which a queued change would otherwise hold `/goal clear` behind.

use std::sync::Arc;

use rho_protocol::{
    methods::{ConfigSet, SessionConfigure, SessionMcp, SessionPrompt},
    ClientEnvironment, ConfigSetParams, ContentBlock, ErrorCode, SessionChange,
    SessionConfigureParams, SessionGoalAction, SessionId, SessionMcpParams, SessionPromptParams,
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

/// A request that must not wait behind a queued change.
enum Control {
    Mcp,
    Goal(SessionGoalAction),
    /// A settings write; `label` names it in a refusal.
    SetConfig {
        params: ConfigSetParams,
        label: String,
    },
}

pub(super) struct DaemonRequests {
    sender: mpsc::UnboundedSender<Request>,
    pub(super) views: mpsc::UnboundedReceiver<DaemonView>,
    task: tokio::task::JoinHandle<()>,
    control: mpsc::UnboundedSender<Control>,
    control_task: tokio::task::JoinHandle<()>,
}

impl DaemonRequests {
    /// `link` must come from `DaemonSession::attached_link`, never reconnect.
    pub(super) fn new(link: Arc<Link>, session: SessionId) -> Self {
        let (sender, mut requests) = mpsc::unbounded_channel();
        let (views, receiver) = mpsc::unbounded_channel();
        let (control, mut control_requests) = mpsc::unbounded_channel();
        let control_link = Arc::clone(&link);
        let control_session = session.clone();
        let control_views = views.clone();
        // Control requests bypass configure, which can wait for a turn to end.
        let control_task = tokio::spawn(async move {
            while let Some(request) = control_requests.recv().await {
                match request {
                    Control::Mcp => {
                        let result = control_link
                            .request::<SessionMcp>(&SessionMcpParams {
                                session: control_session.clone(),
                            })
                            .await
                            .map(Box::new)
                            .map_err(|error| {
                                format!("could not read session MCP inventory: {error}")
                            });
                        let _ = control_views.send(DaemonView::McpInventory(result));
                    }
                    Control::Goal(action) => {
                        let verb = goal_verb(&action);
                        let response = control_link
                            .request::<SessionConfigure>(&SessionConfigureParams {
                                session: control_session.clone(),
                                change: SessionChange::Goal { action },
                            })
                            .await;
                        // The goal's OptionsChanged, not this response, is
                        // authoritative.
                        if let Err(error) = response {
                            let _ = control_views.send(DaemonView::Error(format!(
                                "could not {verb} the goal: {error}"
                            )));
                        }
                    }
                    Control::SetConfig { params, label } => {
                        // `rho/config/changed`, not this response, updates
                        // what the TUI shows.
                        let view = match control_link.request::<ConfigSet>(&params).await {
                            Ok(_) => continue,
                            Err(error) if error.error_code() == Some(ErrorCode::ConfigConflict) => {
                                DaemonView::Status("settings changed elsewhere; try again".into())
                            }
                            Err(error) => {
                                DaemonView::Error(format!("could not save {label}: {error}"))
                            }
                        };
                        let _ = control_views.send(view);
                    }
                }
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
            control,
            control_task,
        }
    }

    pub(super) fn request_mcp(&self) -> anyhow::Result<()> {
        self.control
            .send(Control::Mcp)
            .map_err(|_| anyhow::anyhow!("lost the connection to the rho daemon"))
    }

    pub(super) fn goal(&self, action: SessionGoalAction) -> anyhow::Result<()> {
        self.control
            .send(Control::Goal(action))
            .map_err(|_| anyhow::anyhow!("lost the connection to the rho daemon"))
    }

    /// Writes settings without waiting for the active turn.
    pub(super) fn set_config(&self, params: ConfigSetParams, label: String) -> anyhow::Result<()> {
        self.control
            .send(Control::SetConfig { params, label })
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
        self.control_task.abort();
    }
}

/// The verb a refused goal action's error says, as in `could not set the goal`.
fn goal_verb(action: &SessionGoalAction) -> &'static str {
    match action {
        SessionGoalAction::Set { .. } => "set",
        SessionGoalAction::Clear => "clear",
        SessionGoalAction::Resume => "resume",
        _ => "change",
    }
}
