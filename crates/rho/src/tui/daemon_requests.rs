//! Serial configuration and prompt requests on the attached connection.
//!
//! The queues follow the session's [`Target`]: while the daemon restarts
//! in place (#1713) they wait for the session to attach again, so a change
//! queued meanwhile still applies, in order, on the new connection.
//!
//! Configure can wait for an active turn (including an approval) to end. Its
//! response must never own the TUI event loop. Prompts share the queue so a
//! later prompt cannot overtake a configuration change. A prompt carries an
//! idempotency key, so one a stopping daemon refused goes again on the
//! restarted daemon without running twice (#1777). Cancel and answers
//! deliberately bypass this queue, since they may unblock that change. MCP
//! inventory reads, renames, goal actions, and settings writes use an
//! independent control worker to stay usable during the turn: a goal runs as
//! one long turn, which a queued change would otherwise hold `/goal clear`
//! behind.
//!
//! A change still queued here when the TUI quits is not sent: context from a
//! `!` command that finished behind a change waiting for the turn is lost
//! with the queue.

use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use rho_protocol::{
    methods::{
        ConfigSet, JobList, SessionConfigure, SessionDelete, SessionInspect, SessionMcp,
        SessionPrompt, SessionRename,
    },
    ClientEnvironment, ClientError, ConfigSetParams, ConfigWriter, ContentBlock, ErrorCode,
    IdempotencyKey, JobListParams, SessionChange, SessionConfigureParams, SessionDeleteParams,
    SessionGoalAction, SessionId, SessionInspectParams, SessionMcpParams, SessionPromptParams,
    SessionRenameParams, SessionReport,
};
use tokio::sync::{mpsc, oneshot, watch};

use super::{daemon_events::DaemonView, daemon_options::change_failure};
use crate::app::daemon_client::Link;

enum Request {
    Configure(SessionChange, Asked),
    Prompt {
        content: Vec<ContentBlock>,
        result: oneshot::Sender<anyhow::Result<()>>,
    },
}

/// Who asked for a change, which decides whether its end is reported.
#[derive(Clone, Copy)]
enum Asked {
    /// The app, which counts the change as pending until
    /// [`DaemonView::ConfigureFinished`].
    ByApp,
    /// The launch, carrying a setting to a session opened mid-turn: the
    /// app never counted it, so its end must not settle one the app did.
    AtLaunch,
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
/// is attached on a connection that is still open; `None` once following
/// ended. An attachment whose connection closed waits for what the session
/// makes of that: a reattach after a restart, or the end of following.
async fn attached(target: &mut watch::Receiver<Target>) -> Option<(Arc<Link>, SessionId)> {
    let current = target
        .wait_for(|target| match target {
            Target::Attached { link, .. } => link.is_alive(),
            Target::Reconnecting => false,
            Target::Ended => true,
        })
        .await
        .ok()?;
    match &*current {
        Target::Attached { link, session } => Some((Arc::clone(link), session.clone())),
        Target::Reconnecting | Target::Ended => None,
    }
}

/// Sends a request with `send` on the session's attachment, and returns the
/// connection it went on with the answer; `None` once following ended. The
/// daemon refuses a request as stopping, and applies none of it, once its
/// restart began, which the session hears of only after its detach is read:
/// such a request waits until the session moved off that connection and is
/// sent again, so it applies on the restarted daemon. After a stop, the
/// session's following ends instead.
async fn on_attachment<T, F, Fut>(
    target: &mut watch::Receiver<Target>,
    mut send: F,
) -> Option<(Arc<Link>, Result<T, ClientError>)>
where
    F: FnMut(Arc<Link>, SessionId) -> Fut,
    Fut: Future<Output = Result<T, ClientError>>,
{
    loop {
        let (link, session) = attached(target).await?;
        let answer = send(Arc::clone(&link), session).await;
        match &answer {
            Err(error) if error.error_code() == Some(ErrorCode::DaemonStopping) => {
                target
                    .wait_for(|target| match target {
                        Target::Attached { link: current, .. } => !Arc::ptr_eq(current, &link),
                        Target::Reconnecting | Target::Ended => true,
                    })
                    .await
                    .ok()?;
            }
            _ => return Some((link, answer)),
        }
    }
}

/// A request that must not wait behind a queued change.
enum Control {
    Mcp,
    Goal(SessionGoalAction),
    Rename(String),
    Jobs,
    Inspect(SessionReport),
    /// Deletes saved sessions, in order; `result` takes each one's refusal.
    Delete {
        sessions: Vec<String>,
        result: oneshot::Sender<Vec<Result<(), String>>>,
    },
    /// A settings write; `label` names it in a refusal.
    SetConfig {
        params: ConfigSetParams,
        label: String,
    },
}

/// Deletes saved sessions through the daemon, which can let go of one it
/// holds open: see [`DaemonRequests::deleter`].
#[derive(Clone)]
pub(super) struct SessionDeleter(mpsc::UnboundedSender<Control>);

impl SessionDeleter {
    /// Asks the daemon to delete each of `sessions`; one answer each, in
    /// order, a refusal as the daemon worded it.
    pub(super) async fn delete(&self, sessions: Vec<String>) -> Vec<Result<(), String>> {
        let lost = vec![Err(LOST.to_owned()); sessions.len()];
        let (result, answers) = oneshot::channel();
        if self.0.send(Control::Delete { sessions, result }).is_err() {
            return lost;
        }
        answers.await.unwrap_or(lost)
    }
}

pub(super) struct DaemonRequests {
    sender: mpsc::UnboundedSender<Request>,
    pub(super) views: mpsc::UnboundedReceiver<DaemonView>,
    task: tokio::task::JoinHandle<()>,
    control: mpsc::UnboundedSender<Control>,
    control_task: tokio::task::JoinHandle<()>,
    /// Set once a change saved rows, which saves a session that had
    /// nothing saved yet.
    saved_rows: Arc<AtomicBool>,
}

impl DaemonRequests {
    /// Sends to where `target` says the session is attached, never to a
    /// connection the session has not attached on.
    pub(super) fn new(target: watch::Receiver<Target>) -> Self {
        let (sender, mut requests) = mpsc::unbounded_channel();
        let (views, receiver) = mpsc::unbounded_channel();
        let (control, mut control_requests) = mpsc::unbounded_channel();
        // Names these writes, so the daemon tells them from another
        // client's (see `ConfigSetParams::writer`).
        let writer = ConfigWriter::new(uuid::Uuid::new_v4().to_string());
        let mut control_target = target.clone();
        let mut target = target;
        let control_views = views.clone();
        // Control requests bypass configure, which can wait for a turn to end.
        let control_task = tokio::spawn(async move {
            while let Some(request) = control_requests.recv().await {
                match request {
                    Control::Mcp => {
                        let answer =
                            on_attachment(&mut control_target, |link, session| async move {
                                link.request::<SessionMcp>(&SessionMcpParams { session })
                                    .await
                            })
                            .await;
                        let result = match answer {
                            Some((_, answer)) => answer.map(Box::new).map_err(|error| {
                                format!("could not read session MCP inventory: {error}")
                            }),
                            None => Err(LOST.to_owned()),
                        };
                        let _ = control_views.send(DaemonView::McpInventory(result));
                    }
                    Control::Goal(action) => {
                        let verb = goal_verb(&action);
                        let answer = on_attachment(&mut control_target, |link, session| {
                            let action = action.clone();
                            async move {
                                link.request::<SessionConfigure>(&SessionConfigureParams {
                                    session,
                                    change: SessionChange::Goal { action },
                                })
                                .await
                            }
                        })
                        .await;
                        // The goal's OptionsChanged, not this response, is
                        // authoritative.
                        let error = match answer {
                            Some((_, Ok(_))) => continue,
                            Some((_, Err(error))) => error.to_string(),
                            None => LOST.to_owned(),
                        };
                        let _ = control_views.send(DaemonView::Error(format!(
                            "could not {verb} the goal: {error}"
                        )));
                    }
                    Control::Jobs => {
                        let answer = on_attachment(&mut control_target, |link, _| async move {
                            link.request::<JobList>(&JobListParams {}).await
                        })
                        .await;
                        let jobs = match answer {
                            Some((_, answer)) => answer
                                .map(|answer| answer.jobs)
                                .map_err(|error| error.to_string()),
                            None => Err(LOST.to_owned()),
                        };
                        let _ = control_views.send(DaemonView::Jobs(jobs));
                    }
                    Control::Inspect(report) => {
                        let answer =
                            on_attachment(&mut control_target, |link, session| async move {
                                link.request::<SessionInspect>(&SessionInspectParams {
                                    session,
                                    report,
                                })
                                .await
                            })
                            .await;
                        let result = match answer {
                            Some((_, answer)) => answer
                                .map(|answer| answer.report)
                                .map_err(|error| error.to_string()),
                            None => Err(LOST.to_owned()),
                        };
                        let _ = control_views.send(DaemonView::Report { report, result });
                    }
                    Control::Rename(title) => {
                        let answer = on_attachment(&mut control_target, |link, session| {
                            let title = title.clone();
                            async move {
                                link.request::<SessionRename>(&SessionRenameParams {
                                    session,
                                    title,
                                })
                                .await
                            }
                        })
                        .await;
                        // The session's TitleChanged says the rename landed.
                        let error = match answer {
                            Some((_, Ok(_))) => continue,
                            Some((_, Err(error))) => error.to_string(),
                            None => LOST.to_owned(),
                        };
                        let _ = control_views.send(DaemonView::Error(format!(
                            "could not rename session: {error}"
                        )));
                    }
                    Control::Delete { sessions, result } => {
                        let mut answers = Vec::with_capacity(sessions.len());
                        for session in sessions {
                            // A session this TUI left is no longer attached;
                            // one a client still shows, this TUI's own
                            // included, is not this delete's to take.
                            let params = SessionDeleteParams {
                                session: SessionId::new(session),
                                unless_attached: true,
                            };
                            let answer = on_attachment(&mut control_target, |link, _| {
                                let params = params.clone();
                                async move { link.request::<SessionDelete>(&params).await }
                            })
                            .await;
                            answers.push(match answer {
                                Some((_, Ok(_))) => Ok(()),
                                // The daemon's own words name the session
                                // and what holds it.
                                Some((_, Err(ClientError::Rpc { error, .. }))) => {
                                    Err(error.message)
                                }
                                Some((_, Err(error))) => Err(error.to_string()),
                                None => Err(LOST.to_owned()),
                            });
                        }
                        let _ = result.send(answers);
                    }
                    Control::SetConfig { mut params, label } => {
                        params.writer = Some(writer.clone());
                        let answer = on_attachment(&mut control_target, |link, _| {
                            let params = params.clone();
                            async move { link.request::<ConfigSet>(&params).await }
                        })
                        .await;
                        // `rho/config/changed`, not this response, updates
                        // what the TUI shows.
                        let view = match answer {
                            Some((_, Ok(_))) => continue,
                            Some((_, Err(error)))
                                if error.error_code() == Some(ErrorCode::ConfigConflict) =>
                            {
                                // Another client's change or a hand edit;
                                // this TUI's own writes conflict only
                                // across a daemon restart.
                                DaemonView::Status("settings changed; try again".into())
                            }
                            Some((_, Err(error))) => {
                                DaemonView::Error(format!("could not save {label}: {error}"))
                            }
                            None => DaemonView::Error(format!("could not save {label}: {LOST}")),
                        };
                        let _ = control_views.send(view);
                    }
                }
            }
        });
        let saved_rows = Arc::new(AtomicBool::new(false));
        let saved = Arc::clone(&saved_rows);
        let task = tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                match request {
                    Request::Configure(change, asked) => {
                        let failure = change_failure(&change);
                        let saves_rows = matches!(
                            change,
                            SessionChange::Context { .. } | SessionChange::AddDirectory { .. }
                        );
                        let answer = on_attachment(&mut target, |link, session| {
                            let change = change.clone();
                            async move {
                                link.request::<SessionConfigure>(&SessionConfigureParams {
                                    session,
                                    change,
                                })
                                .await
                            }
                        })
                        .await;
                        let error = match answer {
                            Some((link, answer)) => {
                                link.fence().await;
                                answer.err().map(|error| error.to_string())
                            }
                            None => Some(LOST.to_owned()),
                        };
                        if error.is_none() && saves_rows {
                            saved.store(true, Ordering::Relaxed);
                        }
                        if let Asked::ByApp = asked {
                            let _ = views.send(DaemonView::ConfigureFinished);
                        }
                        if let Some(error) = error {
                            let _ = views.send(DaemonView::Error(format!("{failure}: {error}")));
                        }
                        // OptionsChanged, not an optimistic local update, is
                        // authoritative. The response describes the same change.
                    }
                    // One key for every send of this prompt: a stopping
                    // daemon may refuse a prompt its turn already saved,
                    // and the restarted daemon then answers with that turn
                    // instead of running the prompt again.
                    Request::Prompt { content, result } => {
                        let key = IdempotencyKey::new(uuid::Uuid::new_v4().to_string());
                        let answer = on_attachment(&mut target, |link, session| {
                            let params = SessionPromptParams {
                                session,
                                content: content.clone(),
                                timeout_ms: None,
                                environment: ClientEnvironment::of_this_process(),
                                idempotency_key: Some(key.clone()),
                                grant: None,
                            };
                            async move {
                                let answer = link.request::<SessionPrompt>(&params).await;
                                link.fence().await;
                                answer
                            }
                        })
                        .await;
                        let response = match answer {
                            Some((_, answer)) => answer.map(|_| ()).map_err(anyhow::Error::new),
                            None => Err(anyhow::anyhow!(LOST)),
                        };
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
            saved_rows,
        }
    }

    /// Whether a change sent from here saved rows to the session.
    pub(super) fn saved_rows(&self) -> bool {
        self.saved_rows.load(Ordering::Relaxed)
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

    pub(super) fn inspect(&self, report: SessionReport) -> anyhow::Result<()> {
        self.control
            .send(Control::Inspect(report))
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    pub(super) fn request_jobs(&self) -> anyhow::Result<()> {
        self.control
            .send(Control::Jobs)
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    pub(super) fn rename(&self, title: String) -> anyhow::Result<()> {
        self.control
            .send(Control::Rename(title))
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    /// A handle that deletes sessions through the daemon, for work that
    /// outlives a borrow of the session.
    pub(super) fn deleter(&self) -> SessionDeleter {
        SessionDeleter(self.control.clone())
    }

    /// Writes settings without waiting for the active turn.
    pub(super) fn set_config(&self, params: ConfigSetParams, label: String) -> anyhow::Result<()> {
        self.control
            .send(Control::SetConfig { params, label })
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    pub(super) fn configure(&self, change: SessionChange) -> anyhow::Result<()> {
        self.sender
            .send(Request::Configure(change, Asked::ByApp))
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    /// Queues a launch setting for a session opened mid-turn. A refusal
    /// still shows as an error; nothing reports that it finished.
    pub(super) fn carry(&self, change: SessionChange) -> anyhow::Result<()> {
        self.sender
            .send(Request::Configure(change, Asked::AtLaunch))
            .map_err(|_| anyhow::anyhow!(LOST))
    }

    /// Queues a prompt; its answer arrives on the receiver. Whoever waits
    /// on it must keep reading the session's detaches meanwhile: the prompt
    /// may wait behind a change that waits for the session to reattach.
    pub(super) fn prompt(
        &self,
        content: Vec<ContentBlock>,
    ) -> anyhow::Result<oneshot::Receiver<anyhow::Result<()>>> {
        let (result, response) = oneshot::channel();
        self.sender
            .send(Request::Prompt { content, result })
            .map_err(|_| anyhow::anyhow!(LOST))?;
        Ok(response)
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
