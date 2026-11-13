//! Read the loaded session's background processes without locking its
//! runtime: the rows a client's process rail shows and the output its peek
//! view reads. The session's clients also follow them as
//! [`rho_protocol::SessionEvent::Processes`]: reported before each change
//! of the job's state, and by one watcher per loaded session as a process
//! ends, turn or no turn.

use std::sync::{Arc, PoisonError, Weak};

use rho_protocol::{
    ErrorCode, ProcessChunk, ProcessOutput, ProcessState, ProcessStream, RpcError, SessionProcess,
    SessionProcessesParams, SessionProcessesResponse,
};
use rho_sdk::CancellationToken;
use tokio::sync::Notify;

use super::{LoadedSession, Sessions};
use crate::tools::process::{HostProcessView, LiveProcessSummary, ProcessManager, State, Stream};

/// A session's side of its process watcher.
pub(super) struct ProcessWatch {
    /// Wakes the watcher when the session's process manager may have been
    /// replaced, as a reconfigured tool set does, so it waits on the new one.
    replaced: Arc<Notify>,
    /// Cancelled when the session is dropped.
    dropped: CancellationToken,
}

impl ProcessWatch {
    pub(super) fn new() -> Self {
        Self {
            replaced: Arc::new(Notify::new()),
            dropped: CancellationToken::new(),
        }
    }

    /// Call after replacing the session's process manager.
    pub(super) fn replaced(&self) {
        self.replaced.notify_one();
    }
}

impl Drop for ProcessWatch {
    fn drop(&mut self) {
        self.dropped.cancel();
    }
}

impl LoadedSession {
    /// The session's process manager, when its tools have one.
    fn process_manager(&self) -> Option<ProcessManager> {
        self.processes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Tells the session's clients its background processes, if they
    /// changed since they were last told.
    pub(super) fn report_processes(&self) {
        let processes = self
            .process_manager()
            .map(|manager| {
                manager
                    .live_summaries()
                    .into_iter()
                    .map(process_to_wire)
                    .collect()
            })
            .unwrap_or_default();
        self.feed.update_processes(processes);
    }
}

impl Sessions {
    /// Starts `session`'s process watcher. Call once, as the session is
    /// added.
    pub(super) fn watch_processes(&self, session: &Arc<LoadedSession>) {
        let watch = &session.process_watch;
        let watcher = watch_processes(Arc::downgrade(session), Arc::clone(&watch.replaced));
        let dropped = watch.dropped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = dropped.cancelled() => {}
                () = watcher => {}
            }
        });
    }
}

/// Reports `session`'s processes each time one ends, until the session is
/// gone. Holds the session weakly, so it never keeps one loaded.
async fn watch_processes(session: Weak<LoadedSession>, replaced: Arc<Notify>) {
    loop {
        let exited = {
            let Some(session) = session.upgrade() else {
                return;
            };
            // Subscribed before the report, so an exit in between still
            // wakes the next look.
            let exited = session
                .process_manager()
                .map(|manager| manager.notified_owned());
            session.report_processes();
            exited
        };
        let exited = async move {
            match exited {
                Some(exited) => exited.await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = exited => {}
            () = replaced.notified() => {}
        }
    }
}

impl Sessions {
    pub(in crate::app::daemon_host) async fn processes(
        &self,
        params: SessionProcessesParams,
    ) -> Result<SessionProcessesResponse, RpcError> {
        self.refuse_when_stopping()?;
        let not_loaded = || {
            RpcError::new(
                ErrorCode::SessionNotLoaded,
                "the daemon has not loaded the session; attach to it first",
            )
        };
        let session = self
            .loaded_session(&params.session)
            .await?
            .ok_or_else(not_loaded)?;
        // An idle unload may have taken it since the lookup. A read needs
        // no residency hold: it takes nothing teardown waits on.
        let turn = session.enter().ok_or_else(not_loaded)?;
        let manager = session
            .processes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        drop(turn);
        // A session without the `process` tool has none.
        let Some(manager) = manager else {
            return Ok(SessionProcessesResponse {
                processes: Vec::new(),
                output: None,
            });
        };
        Ok(SessionProcessesResponse {
            processes: manager
                .live_summaries()
                .into_iter()
                .map(process_to_wire)
                .collect(),
            output: params.output.and_then(|output| {
                manager
                    .host_view_from(&output.process, output.from)
                    .ok()
                    .map(output_to_wire)
            }),
        })
    }
}

fn process_to_wire(process: LiveProcessSummary) -> SessionProcess {
    SessionProcess {
        id: process.process_id,
        command: process.command,
        state: state_to_wire(process.state),
        elapsed_seconds: process.elapsed_seconds,
        quiet_seconds: process.quiet_seconds,
        exit_code: process.exit_code,
    }
}

fn output_to_wire(view: HostProcessView) -> ProcessOutput {
    let snapshot = view.snapshot;
    ProcessOutput {
        process: SessionProcess {
            id: snapshot.process_id,
            command: snapshot.command,
            state: state_to_wire(snapshot.state),
            elapsed_seconds: view.elapsed_seconds,
            quiet_seconds: view.quiet_seconds,
            exit_code: snapshot.exit_code,
        },
        first_cursor: snapshot.first_cursor,
        next_cursor: snapshot.next_cursor,
        chunks: snapshot
            .chunks
            .into_iter()
            .map(|chunk| ProcessChunk {
                cursor: chunk.cursor,
                stream: match chunk.stream {
                    Stream::Stdout => ProcessStream::Stdout,
                    Stream::Stderr => ProcessStream::Stderr,
                },
                text: chunk.text,
            })
            .collect(),
    }
}

fn state_to_wire(state: State) -> ProcessState {
    match state {
        State::Starting => ProcessState::Starting,
        State::Running => ProcessState::Running,
        State::Exited => ProcessState::Exited,
        State::Terminated => ProcessState::Terminated,
        State::TimedOut => ProcessState::TimedOut,
        State::FailedToStart => ProcessState::FailedToStart,
    }
}
