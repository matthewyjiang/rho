//! Read the loaded session's background processes without locking its
//! runtime: the rows a client's process rail shows and the output its peek
//! view reads.

use rho_protocol::{
    ErrorCode, ProcessChunk, ProcessOutput, ProcessState, ProcessStream, RpcError, SessionProcess,
    SessionProcessesParams, SessionProcessesResponse,
};

use super::Sessions;
use crate::tools::process::{HostProcessView, LiveProcessSummary, State, Stream};

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
        let manager = session.background.processes();
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
