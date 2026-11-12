//! The daemon client side of `rho attach`: names the session, follows its
//! events, hands text mode's terminal what it answers and steers with, and
//! turns how the turn ended into the process's exit.
//!
//! A subagent or workflow agent node run is followed by the session it
//! writes, which the client observes rather than attaches to (#1668): the
//! run's questions go to its parent session, so there is nothing to answer
//! or steer here.

use std::path::Path;

use rho_protocol::{
    methods::{JobList, SessionAttach, SessionObserve},
    Client, Cursor, DaemonNotification, DetachReason, JobId, JobKind, JobListParams, JobState,
    JobSummary, Replay, SessionAttachParams, SessionAttachResponse, SessionEvent, SessionId,
    SessionObserveParams, SessionSetup,
};

use super::{
    follow::{Ending, Follow},
    terminal::{Line, Lines, Terminal},
    text::TextView,
};
use crate::{
    app::{
        automation::{self, AutomationExit},
        automation_protocol::{JsonlStream, TerminalReason},
        daemon_cli,
        shutdown_signal::ShutdownSignals,
    },
    cli::{AttachInput, AttachOutput},
    daemon::{self, ConnectAs, Connection},
};

pub(in crate::app) async fn run(
    id: &str,
    output: AttachOutput,
    input: AttachInput,
) -> anyhow::Result<()> {
    let mut view = View::new(output);
    let exit = attach(id, &mut view, Lines::of(input)).await;
    Ok(view.end(exit)?)
}

/// Where `rho attach` writes the session's events.
enum View {
    Text(TextView),
    Jsonl(JsonlStream),
}

impl View {
    fn new(output: AttachOutput) -> Self {
        match output {
            AttachOutput::Text => Self::Text(TextView::terminal()),
            AttachOutput::Jsonl => Self::Jsonl(JsonlStream::stdout()),
        }
    }

    /// The terminal that tells and answers the session's input requests:
    /// text output's; JSONL readers answer through the protocol. None for
    /// a run, which asks its parent session.
    fn terminal(&self, target: &Target, lines: Lines) -> Result<Option<Terminal>, AutomationExit> {
        match (self, target) {
            (Self::Text(_), Target::Session(_)) => Terminal::open(lines).map(Some),
            (Self::Text(_), Target::Run { .. }) | (Self::Jsonl(_), _) => Ok(None),
        }
    }

    /// Starts the output for `session`, which runs in `workspace`.
    fn started(&mut self, session: &SessionId, workspace: &Path) -> Result<(), AutomationExit> {
        match self {
            Self::Text(_) => Ok(()),
            Self::Jsonl(stream) => {
                stream.run_started(session.as_str(), workspace);
                automation::stream_written(stream)
            }
        }
    }

    /// Starts the output over for a reset replay.
    fn restart(&mut self) -> Result<(), AutomationExit> {
        match self {
            Self::Text(view) => view.restart().map_err(automation::write_failed),
            // JSONL readers see the replay's events as they come.
            Self::Jsonl(_) => Ok(()),
        }
    }

    fn show(&mut self, event: &SessionEvent) -> Result<(), AutomationExit> {
        match self {
            Self::Text(view) => view.show(event).map_err(automation::write_failed),
            Self::Jsonl(stream) => {
                stream.event(event);
                automation::stream_written(stream)
            }
        }
    }

    /// Ends the output as the attachment ended with `exit`, and returns the
    /// exit, unless the output could not be written.
    fn end(&mut self, exit: Result<(), AutomationExit>) -> Result<(), AutomationExit> {
        match self {
            Self::Text(view) => {
                // An ending without `turn_ended`, such as a detach, can stop
                // mid-reply; a terminal that cannot take the newline either
                // already failed the output or ends here anyway.
                let _ = view.finish();
                exit
            }
            Self::Jsonl(stream) => automation::end_stream(stream, exit),
        }
    }
}

/// Follows the session `id` names until its turn ends, writing its events
/// to `view`, and returns the exit that end makes. A signal detaches at any
/// point, connecting included.
async fn attach(id: &str, view: &mut View, lines: Lines) -> Result<(), AutomationExit> {
    let mut signals = ShutdownSignals::install().map_err(|error| {
        AutomationExit::other(format!("could not install signal handlers: {error}"))
    })?;
    tokio::select! {
        exit = follow(id, view, lines) => exit,
        signal = signals.recv() => {
            let signal =
                signal.map_err(|error| AutomationExit::other(format!("could not wait for signals: {error}")))?;
            Err(AutomationExit::new(
                signal.exit_code(),
                TerminalReason::Detached,
                format!("detached on {signal}; the turn runs on in the daemon"),
            ))
        }
    }
}

async fn follow(id: &str, view: &mut View, lines: Lines) -> Result<(), AutomationExit> {
    let paths = daemon_cli::paths().map_err(|error| AutomationExit::other(format!("{error:#}")))?;
    let Connection {
        client,
        mut notifications,
        ..
    } = daemon::connect(&paths, ConnectAs::Frontend.hello())
        .await
        .map_err(|error| AutomationExit::other(format!("{error:#}")))?;
    let jobs = client
        .request::<JobList>(&JobListParams {})
        .await
        .map_err(|error| {
            AutomationExit::other(format!("could not list the daemon's jobs: {error}"))
        })?
        .jobs;
    let target = resolve(&jobs, id)?;
    let session = target.session().clone();
    let response = match (attach_session(&client, &target, None).await, &target) {
        (Ok(response), _) => response,
        // A run that ended before it opened its session, as one cancelled
        // while queued, left nothing to follow or replay.
        (Err(refused), Target::Run { job, .. }) => {
            return match run_end(&client, job).await? {
                Some(end) => end,
                None => Err(refused),
            }
        }
        (Err(refused), Target::Session(_)) => return Err(refused),
    };
    view.started(&session, &response.workspace)?;
    let (mut follow, ending) = Follow::attached(&response);
    if let Some(ending) = ending {
        return ended(ending);
    }
    let mut terminal = view.terminal(&target, lines)?;
    // The response's open requests, held until its replay is written, so a
    // prompt follows the conversation it belongs to; lines wait with them.
    let mut pending = Some(response);
    loop {
        if let Some(terminal) = terminal.as_mut().filter(|_| !follow.replaying()) {
            if let Some(response) = pending.take() {
                terminal.attached(&response.inputs, response.replay)?;
            }
        }
        let ending = tokio::select! {
            notification = notifications.recv() => match notification {
                None => return Err(AutomationExit::other("lost the connection to the rho daemon")),
                Some(DaemonNotification::SessionEvent(params))
                    if params.session == session =>
                {
                    view.show(&params.event)?;
                    // A replayed event is history the attach response's open
                    // requests already account for.
                    if let Some(terminal) = terminal.as_mut().filter(|_| !follow.replaying()) {
                        terminal.event(&params.event)?;
                    }
                    follow.observe(params.seq, &params.event)
                }
                Some(DaemonNotification::SessionDetached(params))
                    if params.session == session =>
                {
                    match params.reason {
                        // Resumes after the last event seen, or starts over
                        // when the daemon no longer buffers it, as it no
                        // longer does for a session it unloaded.
                        DetachReason::Lagged => {
                            let response =
                                attach_session(&client, &target, follow.cursor()).await?;
                            if response.replay == Replay::Reset {
                                view.restart()?;
                            }
                            let ending = follow.reattached(&response);
                            pending = Some(response);
                            ending
                        }
                        // A restart interrupts the turn as a stop does; an
                        // upgrade waits for it to end first, by when this
                        // has ended too.
                        DetachReason::DaemonStopping | DetachReason::Restarting => {
                            Some(Ending::Interrupted)
                        }
                        DetachReason::Deleted => {
                            return Err(AutomationExit::other("the session was deleted"))
                        }
                        // The run ended before this followed it: what came
                        // was its session as saved, with no turn's end.
                        DetachReason::NotLoaded => match &target {
                            Target::Run { job, .. } => {
                                return run_end(&client, job).await?.unwrap_or_else(|| {
                                    Err(AutomationExit::other(
                                        "the rho daemon detached this client",
                                    ))
                                })
                            }
                            Target::Session(_) => {
                                return Err(AutomationExit::other(
                                    "the rho daemon detached this client",
                                ))
                            }
                        },
                        // `DetachReason` is non-exhaustive.
                        _ => return Err(AutomationExit::other("the rho daemon detached this client")),
                    }
                }
                // Another session's, or a notification a newer daemon sends.
                Some(_) => None,
            },
            typed = next_line(&mut terminal), if pending.is_none() => {
                let (terminal, line) = typed?;
                terminal.line(&client, &session, &line).await?;
                None
            }
        };
        if let Some(ending) = ending {
            return ended(ending);
        }
    }
}

/// The next line typed at `terminal`, with it; never ready without one.
async fn next_line(
    terminal: &mut Option<Terminal>,
) -> Result<(&mut Terminal, Line), AutomationExit> {
    match terminal {
        Some(terminal) => {
            let line = terminal.next_line().await?;
            Ok((terminal, line))
        }
        None => std::future::pending().await,
    }
}

/// What `rho attach` follows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    /// A session, attached to: its requests are answered and its turn
    /// steered from here.
    Session(SessionId),
    /// Subagent job `job`'s run, observed by the `session` it writes.
    Run { job: JobId, session: SessionId },
}

impl Target {
    fn session(&self) -> &SessionId {
        match self {
            Self::Session(session) | Self::Run { session, .. } => session,
        }
    }
}

async fn attach_session(
    client: &Client,
    target: &Target,
    after: Option<&Cursor>,
) -> Result<SessionAttachResponse, AutomationExit> {
    let attached = match target {
        Target::Session(session) => {
            let params = SessionAttachParams {
                session: session.clone(),
                after: after.cloned(),
                // Only a session the attach reopens takes it (#1659).
                environment: rho_protocol::ClientEnvironment::of_this_process(),
                setup: SessionSetup::default(),
            };
            client.request::<SessionAttach>(&params).await
        }
        Target::Run { session, .. } => {
            let params = SessionObserveParams {
                session: session.clone(),
                after: after.cloned(),
            };
            client.request::<SessionObserve>(&params).await
        }
    };
    attached.map_err(|error| {
        AutomationExit::other(format!(
            "could not attach to session {}: {error}",
            target.session()
        ))
    })
}

/// The exit for a run that had ended by the time it was followed, by where
/// its job `job` stands; `None` while the run is still under way.
async fn run_end(
    client: &Client,
    job: &JobId,
) -> Result<Option<Result<(), AutomationExit>>, AutomationExit> {
    let jobs = client
        .request::<JobList>(&JobListParams {})
        .await
        .map_err(|error| {
            AutomationExit::other(format!("could not list the daemon's jobs: {error}"))
        })?
        .jobs;
    let ended = |how: &str| Some(Err(AutomationExit::other(format!("job {job}'s run {how}"))));
    Ok(match jobs.iter().find(|listed| &listed.id == job) {
        Some(listed) if listed.state.in_progress() => None,
        Some(JobSummary {
            state: JobState::Completed,
            ..
        }) => Some(Ok(())),
        Some(JobSummary {
            state: JobState::Cancelled,
            ..
        }) => ended("was cancelled"),
        Some(JobSummary {
            error: Some(error), ..
        }) => ended(&format!("failed: {error}")),
        Some(_) | None => ended("ended without completing"),
    })
}

/// What `id` names: a session job's id, or a session id, for its session;
/// a subagent job's id, or the id of the session its run writes, for the
/// run. A session no job of the daemon's ran, such as one the TUI saved, is
/// named by its own id, and the daemon reopens it (#1661). A session id
/// several jobs wrote names the latest of them. A run on another
/// runtime keeps no Rho session, and a workflow run's drive has no events
/// of its own, only its agent nodes' runs: both are refused, naming the
/// session they run in.
fn resolve(jobs: &[JobSummary], id: &str) -> Result<Target, AutomationExit> {
    // The latest: a workflow node that runs again writes its session under
    // a newer job, and the list is oldest first.
    let Some(job) = jobs.iter().rev().find(|job| {
        job.id.as_str() == id || job.session.as_ref().is_some_and(|s| s.as_str() == id)
    }) else {
        return Ok(Target::Session(SessionId::new(id)));
    };
    let why = match (job.kind, &job.session) {
        (JobKind::Session, Some(session)) => return Ok(Target::Session(session.clone())),
        (JobKind::Session, None) => {
            return Err(AutomationExit::other(format!(
                "session job {} has no session",
                job.id
            )))
        }
        (JobKind::Subagent, Some(session)) => {
            // The session's stream is its latest run's.
            let latest = jobs
                .iter()
                .rev()
                .find(|other| other.session.as_ref() == Some(session))
                .unwrap_or(job);
            if latest.id != job.id {
                return Err(AutomationExit::other(format!(
                    "job {} ran before job {} in the same session; attach to that one",
                    job.id, latest.id
                )));
            }
            return Ok(Target::Run {
                job: job.id.clone(),
                session: session.clone(),
            });
        }
        (JobKind::Subagent, None) => {
            "is a subagent run on another runtime, which keeps no rho session to follow"
        }
        (JobKind::Workflow, _) => "drives a workflow run, whose events are its agent nodes' jobs'",
        // `JobKind` is non-exhaustive.
        _ => {
            return Err(AutomationExit::other(format!(
                "job {} is of a kind this rho cannot attach to",
                job.id
            )))
        }
    };
    let place = match session_of(jobs, job) {
        Some(session) => format!("; it runs in session {session}"),
        None => String::new(),
    };
    Err(AutomationExit::other(format!(
        "job {} {why}{place}",
        job.id
    )))
}

/// The session `job` runs in: its nearest session job ancestor's.
fn session_of<'a>(jobs: &'a [JobSummary], job: &JobSummary) -> Option<&'a SessionId> {
    let mut parent = job.parent.as_ref()?;
    // Each step goes up one job, so a chain is never longer than the list,
    // even one a confused daemon made circular.
    for _ in 0..jobs.len() {
        let ancestor = jobs.iter().find(|job| &job.id == parent)?;
        if ancestor.kind == JobKind::Session {
            return ancestor.session.as_ref();
        }
        parent = ancestor.parent.as_ref()?;
    }
    None
}

/// The exit an attachment that ended as `ending` makes.
fn ended(ending: Ending) -> Result<(), AutomationExit> {
    match ending {
        Ending::Turn(outcome) => automation::outcome_exit(&outcome),
        Ending::Interrupted => Err(AutomationExit::new(
            1,
            TerminalReason::Interrupted,
            "the rho daemon stopped before the turn ended",
        )),
        Ending::Unreported => Err(AutomationExit::other(
            "the rho daemon's replay ended after the turn without saying how it ended",
        )),
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
