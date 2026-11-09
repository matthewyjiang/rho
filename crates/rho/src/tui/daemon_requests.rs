//! Serial configuration and prompt requests on the attached connection.
//!
//! Configure can wait for an active turn (including an approval) to end. Its
//! response must never own the TUI event loop. Prompts share the queue so a
//! later prompt cannot overtake a configuration change. Cancel and answers
//! deliberately bypass this queue, since they may unblock that change. MCP
//! inventory reads, goal actions, and settings writes use an independent
//! control worker to stay usable during the turn: a goal runs as one long
//! turn, which a queued change would otherwise hold `/goal clear` behind.

use std::{collections::HashMap, sync::Arc};

use rho_protocol::{
    methods::{ConfigSet, SessionConfigure, SessionMcp, SessionPrompt},
    ClientEnvironment, ConfigNamespace, ConfigRevision, ConfigSetParams, ContentBlock, ErrorCode,
    SessionChange, SessionConfigureParams, SessionGoalAction, SessionId, SessionMcpParams,
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

/// The revision each namespace's last write of this TUI produced, by the
/// revision the TUI stamped it with. A quick second write still carries
/// that stamp, since the daemon's `rho/config/changed` for the first has
/// not reached the cache yet; it is sent against the first write's result
/// instead, so it is not refused as a conflict with the TUI's own change.
/// A change by anyone else still makes the daemon refuse it.
#[derive(Default)]
struct OwnWrites(HashMap<ConfigNamespace, (ConfigRevision, ConfigRevision)>);

impl OwnWrites {
    fn revision_for(&self, namespace: &ConfigNamespace, stamped: ConfigRevision) -> ConfigRevision {
        match self.0.get(namespace) {
            Some((base, saved)) if *base == stamped => saved.clone(),
            _ => stamped,
        }
    }

    /// Records that a write stamped `stamped` saved `saved`. A write that
    /// was itself chained keeps the original stamp, which is what the
    /// cache still holds.
    fn saved(
        &mut self,
        namespace: ConfigNamespace,
        stamped: ConfigRevision,
        saved: ConfigRevision,
    ) {
        self.0.insert(namespace, (stamped, saved));
    }
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
            let mut writes = OwnWrites::default();
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
                    Control::SetConfig { mut params, label } => {
                        // `rho/config/changed`, not this response, updates
                        // what the TUI shows; the response only lets the
                        // next write build on this one.
                        let base = params.revision.clone();
                        params.revision = writes.revision_for(&params.namespace, base.clone());
                        let view = match control_link.request::<ConfigSet>(&params).await {
                            Ok(response) => {
                                writes.saved(params.namespace, base, response.revision);
                                continue;
                            }
                            Err(error) if error.error_code() == Some(ErrorCode::ConfigConflict) => {
                                // Another client's change, or this TUI's own
                                // write the daemon has not reported back yet.
                                DaemonView::Status("settings changed; try again".into())
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

#[cfg(test)]
#[path = "daemon_requests_tests.rs"]
mod tests;
