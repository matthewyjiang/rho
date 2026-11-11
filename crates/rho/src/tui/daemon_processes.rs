//! The process rail and peek view of a daemon session: the background
//! processes its `process` tool started, which the daemon's session owns
//! and reports (`rho/session/processes`). The rail asks for them on the
//! rails' schedule (`daemon_jobs`), and with them for the new output of
//! the process the peek view shows.
//!
//! The daemon lists what the in-process rail shows, so a finished row
//! leaves once its result reached the model, as in process.

use rho_protocol::{
    ProcessOutput, ProcessOutputParams, ProcessState, ProcessStream, SessionProcess,
    SessionProcessesResponse,
};

use super::App;
use crate::tools::process::{Chunk, HostProcessView, LiveProcessSummary, Snapshot, State, Stream};

impl App {
    /// Asks the daemon for the session's processes when the rail is due a
    /// refresh. The answer arrives as a view; see
    /// [`Self::show_daemon_processes`].
    pub(super) fn refresh_daemon_processes(&mut self) {
        let Some(session) = &self.info.session.daemon else {
            return;
        };
        let output = self
            .exclusive
            .peek_view()
            .and_then(|view| view.daemon_cursor())
            .map(|(process, from)| ProcessOutputParams {
                process: process.to_owned(),
                from,
            });
        // A process starts only in a turn, and changes only while its row
        // shows or the peek view follows it.
        let may_change = self.turn.is_provider_turn() || output.is_some();
        let poll = &mut self.daemon_rail.processes;
        if poll.due(may_change) && session.request_processes(output).is_ok() {
            poll.asked();
        }
    }

    /// Puts the session's processes on the process rail, and the output
    /// `asked` for in the peek view when it still reads on from there. A
    /// failed read keeps the rows the rail has and is asked for again.
    pub(super) fn show_daemon_processes(
        &mut self,
        asked: Option<ProcessOutputParams>,
        result: Result<SessionProcessesResponse, String>,
    ) {
        let Ok(answer) = result else {
            self.daemon_rail.processes.answered(/*shows*/ true);
            return;
        };
        let processes: Vec<_> = answer
            .processes
            .into_iter()
            .filter_map(process_summary)
            .collect();
        self.daemon_rail.processes.answered(!processes.is_empty());
        self.process_panel.ingest(processes);
        let (Some(asked), Some(output)) = (asked, answer.output) else {
            return;
        };
        // The person may have left the view, or opened another, since.
        if let Some(view) = self
            .exclusive
            .peek_view_mut()
            .filter(|view| view.daemon_cursor() == Some((asked.process.as_str(), asked.from)))
        {
            if let Some(next) = host_view(output) {
                view.extend(next);
            }
        }
    }
}

/// `process` as a rail row; `None` in a state this build does not know.
fn process_summary(process: SessionProcess) -> Option<LiveProcessSummary> {
    Some(LiveProcessSummary {
        state: process_state(process.state)?,
        process_id: process.id,
        command: process.command,
        elapsed_seconds: process.elapsed_seconds,
        quiet_seconds: process.quiet_seconds,
        exit_code: process.exit_code,
    })
}

fn process_state(state: ProcessState) -> Option<State> {
    match state {
        ProcessState::Starting => Some(State::Starting),
        ProcessState::Running => Some(State::Running),
        ProcessState::Exited => Some(State::Exited),
        ProcessState::Terminated => Some(State::Terminated),
        ProcessState::TimedOut => Some(State::TimedOut),
        ProcessState::FailedToStart => Some(State::FailedToStart),
        ProcessState::Unknown => None,
        // A state this build does not know cannot be shown.
        _ => None,
    }
}

/// `output` as the peek view reads a process; `None` in a state this build
/// does not know.
fn host_view(output: ProcessOutput) -> Option<HostProcessView> {
    let process = process_summary(output.process)?;
    Some(HostProcessView {
        snapshot: Snapshot {
            process_id: process.process_id,
            command: process.command,
            state: process.state,
            runtime_seconds: process.elapsed_seconds as f64,
            first_cursor: output.first_cursor,
            next_cursor: output.next_cursor,
            available_cursor: output.next_cursor,
            truncated: output.first_cursor > 0,
            output_pending: false,
            chunks: output
                .chunks
                .into_iter()
                .map(|chunk| Chunk {
                    cursor: chunk.cursor,
                    stream: match chunk.stream {
                        ProcessStream::Stderr => Stream::Stderr,
                        // Output on a stream this build does not know
                        // still reads as output.
                        ProcessStream::Stdout | ProcessStream::Unknown => Stream::Stdout,
                        _ => Stream::Stdout,
                    },
                    text: chunk.text,
                })
                .collect(),
            exit_code: process.exit_code,
            terminal_detail: None,
        },
        elapsed_seconds: process.elapsed_seconds,
        quiet_seconds: process.quiet_seconds,
    })
}
