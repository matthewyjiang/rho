//! Serial configuration and prompt requests on the attached connection.
//!
//! The queues follow the session's [`Target`]: while the daemon restarts
//! in place (#1713) they wait for the session to attach again, so a change
//! queued meanwhile still applies, in order, on the new connection.
//!
//! Configure can wait for an active turn (including an approval) to end. Its
//! response must never own the TUI event loop. Prompts share the queue so a
//! later prompt cannot overtake a configuration change. Cancel and answers
//! deliberately bypass this queue, since they may unblock that change. MCP
//! inventory reads and goal actions use an independent control worker to stay
//! usable during the turn: a goal runs as one long turn, which a queued
//! change would otherwise hold `/goal clear` behind.

use std::sync::Arc;

use rho_protocol::{
    methods::{SessionConfigure, SessionMcp, SessionPrompt},
    ClientEnvironment, ContentBlock, SessionChange, SessionConfigureParams, SessionGoalAction,
    SessionId, SessionMcpParams, SessionPromptParams,
};
use tokio::sync::{mpsc, oneshot, watch};

use super::{daemon_events::DaemonView, daemon_options::setting_name};
use crate::app::daemon_client::Link;

enum Request {
    Configure(SessionChange),
    Prompt {
        content: Vec<ContentBlock>,
        result: oneshot::Sender<anyhow::Result<()>>,
    },
}

/// Where the session's requests go, as the session publishes it.
#[derive(Clone)]
pub(super) enum Target {
    /// The connection the session is attached on, and the session's id.
    Attached { link: Arc<Link>, session: SessionId },
    /// The daemon restarts: requests wait until the session attached again.
    Reconnecting,
    /// Following ended: requests fail.
    Ended,
}

const LOST: &str = "lost the connection to the rho daemon";

/// The connection and session the next request goes to, once the session
/// is attached; `None` once following ended.
async fn attached(target: &mut watch::Receiver<Target>) -> Option<(Arc<Link>, SessionId)> {
    let current = target
        .wait_for(|target| !matches!(target, Target::Reconnecting))
        .await
        .ok()?;
    match &*current {
        Target::Attached { link, session } => Some((Arc::clone(link), session.clone())),
        Target::Reconnecting | Target::Ended => None,
    }
}

/// A request that must not wait behind a queued change.
enum Control {
    Mcp,
    Goal(SessionGoalAction),
}

pub(super) struct DaemonRequests {
    sender: mpsc::UnboundedSender<Request>,
    pub(super) views: mpsc::UnboundedReceiver<DaemonView>,
    task: tokio::task::JoinHandle<()>,
    control: mpsc::UnboundedSender<Control>,
    control_task: tokio::task::JoinHandle<()>,
}

impl DaemonRequests {
    /// Sends to where `target` says the session is attached, never to a
    /// connection the session has not attached on.
    pub(super) fn new(target: watch::Receiver<Target>) -> Self {
        let (sender, mut requests) = mpsc::unbounded_channel();
        let (views, receiver) = mpsc::unbounded_channel();
        let (control, mut control_requests) = mpsc::unbounded_channel();
        let mut control_target = target.clone();
        let mut target = target;
        let control_views = views.clone();
        // Control requests bypass configure, which can wait for a turn to end.
        let control_task = tokio::spawn(async move {
            while let Some(request) = control_requests.recv().await {
                let Some((control_link, control_session)) = attached(&mut control_target).await
                else {
                    let _ = control_views.send(DaemonView::Error(LOST.to_owned()));
                    continue;
                };
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
                }
            }
        });
        let task = tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                let Some((link, session)) = attached(&mut target).await else {
                    match request {
                        Request::Configure(_) => {
                            let _ = views.send(DaemonView::ConfigureFinished);
                            let _ = views.send(DaemonView::Error(LOST.to_owned()));
                        }
                        Request::Prompt { result, .. } => {
                            let _ = result.send(Err(anyhow::anyhow!(LOST)));
                        }
                    }
                    continue;
                };
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
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    pub(super) fn goal(&self, action: SessionGoalAction) -> anyhow::Result<()> {
        self.control
            .send(Control::Goal(action))
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    pub(super) fn configure(&self, change: SessionChange) -> anyhow::Result<()> {
        self.sender
            .send(Request::Configure(change))
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    pub(super) async fn prompt(&self, content: Vec<ContentBlock>) -> anyhow::Result<()> {
        let (result, response) = oneshot::channel();
        self.sender
            .send(Request::Prompt { content, result })
            .map_err(|_| anyhow::anyhow!(LOST))?;
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
