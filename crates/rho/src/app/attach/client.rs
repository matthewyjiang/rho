//! The daemon client side of `rho attach`: names the session, follows its
//! events, hands text mode's terminal what it answers and steers with, and
//! turns how the turn ended into the process's exit.

use std::path::{Path, PathBuf};

use rho_protocol::{
    methods::{JobList, SessionAttach},
    Client, Cursor, DaemonNotification, DetachReason, ErrorCode, JobKind, JobListParams,
    JobSummary, Replay, SessionAttachParams, SessionAttachResponse, SessionEvent, SessionId,
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
    daemon::{self, cli_client_info, Connection},
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
    /// text output's; JSONL readers answer through the protocol.
    fn terminal(&self, lines: Lines) -> Result<Option<Terminal>, AutomationExit> {
        match self {
            Self::Text(_) => Terminal::open(lines).map(Some),
            Self::Jsonl(_) => Ok(None),
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

/// The session an attachment follows.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    session: SessionId,
    workspace: PathBuf,
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
    } = daemon::connect(&paths, cli_client_info())
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
    view.started(&target.session, &target.workspace)?;
    let response = attach_session(&client, &target.session, None).await?;
    let (mut follow, ending) = Follow::attached(&response);
    if let Some(ending) = ending {
        return ended(ending);
    }
    let mut terminal = view.terminal(lines)?;
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
                    if params.session == target.session =>
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
                    if params.session == target.session =>
                {
                    match params.reason {
                        // Resumes after the last event seen, or starts over
                        // when the daemon no longer buffers it.
                        DetachReason::Lagged => {
                            let response =
                                attach_session(&client, &target.session, follow.cursor()).await?;
                            if response.replay == Replay::Reset {
                                view.restart()?;
                            }
                            let ending = follow.reattached(&response);
                            pending = Some(response);
                            ending
                        }
                        DetachReason::DaemonStopping => Some(Ending::Interrupted),
                        // `DetachReason` is non-exhaustive.
                        _ => return Err(AutomationExit::other("the rho daemon detached this client")),
                    }
                }
                // Another session's, or a notification a newer daemon sends.
                Some(_) => None,
            },
            typed = next_line(&mut terminal), if pending.is_none() => {
                let (terminal, line) = typed?;
                terminal.line(&client, &target.session, &line).await?;
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

async fn attach_session(
    client: &Client,
    session: &SessionId,
    after: Option<&Cursor>,
) -> Result<SessionAttachResponse, AutomationExit> {
    let params = SessionAttachParams {
        session: session.clone(),
        after: after.cloned(),
    };
    client
        .request::<SessionAttach>(&params)
        .await
        .map_err(|error| {
            if error.error_code() == Some(ErrorCode::SessionNotLoaded) {
                AutomationExit::other(format!(
                    "could not attach to session {session}: this rho daemon has not loaded it; a session an earlier daemon ran cannot be followed yet (#1661)"
                ))
            } else {
                AutomationExit::other(format!("could not attach to session {session}: {error}"))
            }
        })
}

/// The session `id` names among `jobs`: a session job's id, or its
/// session's. Subagent and workflow runs do not stream their events yet
/// (#1668), so a job of theirs is refused, naming the session it runs in.
fn resolve(jobs: &[JobSummary], id: &str) -> Result<Target, AutomationExit> {
    let job = jobs
        .iter()
        .find(|job| job.id.as_str() == id || job.session.as_ref().is_some_and(|s| s.as_str() == id))
        .ok_or_else(|| {
            AutomationExit::other(format!("the rho daemon has no job or session {id}"))
        })?;
    let run = match job.kind {
        JobKind::Session => {
            let session = job.session.clone().ok_or_else(|| {
                AutomationExit::other(format!("session job {} has no session", job.id))
            })?;
            return Ok(Target {
                session,
                workspace: job.workspace.clone(),
            });
        }
        JobKind::Subagent => "a subagent run",
        JobKind::Workflow => "a workflow run",
        // `JobKind` is non-exhaustive.
        _ => {
            return Err(AutomationExit::other(format!(
                "job {} is of a kind this rho cannot attach to",
                job.id
            )))
        }
    };
    let place = match session_of(jobs, job) {
        Some(session) => format!(" in session {session}; attach to the session"),
        None => String::new(),
    };
    Err(AutomationExit::other(format!(
        "job {} is {run}{place}: subagent and workflow runs do not stream their events yet (#1668)",
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
