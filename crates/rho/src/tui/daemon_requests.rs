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

/// The revisions this TUI's own writes produced in each namespace, so a
/// write sent before the daemon's `rho/config/changed` for the last one
/// reached the cache does not repeat a stale revision and get refused as a
/// conflict with the TUI's own change. While the cache holds any revision
/// of the chain, everything after it is this TUI's own, so the write is
/// sent against the latest. A change by anyone else breaks the chain: the
/// next own write that still saves starts a new one, and the daemon
/// refuses any write made against the older state. Revisions are content
/// hashes, so another client can land the cache on one the chain holds;
/// the remapped write is then refused and the chain dropped.
#[derive(Default)]
struct OwnWrites(HashMap<ConfigNamespace, OwnChain>);

struct OwnChain {
    /// Every revision of the chain, oldest first; the last is the latest.
    revisions: Vec<ConfigRevision>,
}

impl OwnWrites {
    /// The revision to send a write the TUI stamped `stamped` against.
    fn revision_for(
        &mut self,
        namespace: &ConfigNamespace,
        stamped: ConfigRevision,
    ) -> ConfigRevision {
        let Some(chain) = self.0.get_mut(namespace) else {
            return stamped;
        };
        let Some(seen) = chain
            .revisions
            .iter()
            .position(|revision| *revision == stamped)
        else {
            return stamped;
        };
        // The cache has seen up to `seen`; nothing before it is needed.
        chain.revisions.drain(..seen);
        chain.revisions.last().cloned().unwrap_or(stamped)
    }

    /// Drops the chain of a namespace whose write was refused, so a
    /// stale tip is never sent again.
    fn forget(&mut self, namespace: &ConfigNamespace) {
        self.0.remove(namespace);
    }

    /// Records that a write sent against `sent` saved `saved`.
    fn saved(&mut self, namespace: ConfigNamespace, sent: ConfigRevision, saved: ConfigRevision) {
        let chain = self.0.entry(namespace).or_insert_with(|| OwnChain {
            revisions: Vec::new(),
        });
        if chain.revisions.last() != Some(&sent) {
            chain.revisions = vec![sent];
        }
        chain.revisions.push(saved);
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
                        params.revision =
                            writes.revision_for(&params.namespace, params.revision.clone());
                        let view = match control_link.request::<ConfigSet>(&params).await {
                            Ok(response) => {
                                writes.saved(params.namespace, params.revision, response.revision);
                                continue;
                            }
                            Err(error) if error.error_code() == Some(ErrorCode::ConfigConflict) => {
                                // Another client's change. Revisions are
                                // content hashes, so the cache can hold one
                                // the chain saw without it being this TUI's;
                                // the retry goes out against the cache.
                                writes.forget(&params.namespace);
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
