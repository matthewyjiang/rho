//! Rho's background processes as ACP v2 background work.
//!
//! Each process the session's `process` tool started is a v2 terminal, named
//! by the process id. A terminal is announced (`terminal_update` with its
//! command) before the first `state_update` that lists it, and gets its
//! `exitStatus` before the first one that no longer does. Rho reports
//! background work on every `state_update`, so each one carries the list,
//! empty when nothing runs.
//!
//! The daemon keeps listing a process that ended until its model is given
//! the result; such a process is no longer running, so it is finalized and
//! left out here.

use agent_client_protocol::v2::schema as v2;
use rho_protocol::{ProcessState, SessionProcess};

/// Whether `process` still runs. A state a newer daemon sends counts as
/// running, since unknown work must not look finished.
fn running(process: &SessionProcess) -> bool {
    match process.state {
        ProcessState::Exited
        | ProcessState::Terminated
        | ProcessState::TimedOut
        | ProcessState::FailedToStart => false,
        ProcessState::Starting | ProcessState::Running | ProcessState::Unknown => true,
        _ => true,
    }
}

/// `running`, listing the running `processes`.
pub(super) fn running_state(processes: &[SessionProcess]) -> v2::SessionUpdate {
    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Running(
        v2::RunningStateUpdate::new().background(listed(processes)),
    ))
}

/// `idle`, with why foreground work stopped when it just did, listing the
/// running `processes`.
pub(super) fn idle_state(
    stop_reason: Option<v2::StopReason>,
    processes: &[SessionProcess],
) -> v2::SessionUpdate {
    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(
        v2::IdleStateUpdate::new()
            .stop_reason(stop_reason)
            .background(listed(processes)),
    ))
}

/// The background list of a `state_update`: the running processes.
fn listed(processes: &[SessionProcess]) -> Vec<v2::BackgroundWork> {
    processes
        .iter()
        .filter(|process| running(process))
        .map(|process| v2::BackgroundWork::Terminal(v2::BackgroundTerminal::new(&*process.id)))
        .collect()
}

/// The `terminal_update`s that bring a host that saw `previous` to
/// `current`: an announcement for each process that started running, and
/// the exit status of each that stopped. One that left the list without
/// being seen to end exited with no known status.
pub(super) fn changes(
    previous: &[SessionProcess],
    current: &[SessionProcess],
) -> Vec<v2::SessionUpdate> {
    let was_running = |id: &str| {
        previous
            .iter()
            .any(|process| process.id == id && running(process))
    };
    let started = current
        .iter()
        .filter(|process| running(process) && !was_running(&process.id))
        .map(announcement);
    let stopped = previous
        .iter()
        .filter(|process| running(process))
        .filter_map(|before| {
            match current.iter().find(|process| process.id == before.id) {
                Some(now) if running(now) => None,
                Some(now) => Some(exit(&now.id, now.exit_code)),
                None => Some(exit(&before.id, /*exit_code*/ None)),
            }
        });
    started.chain(stopped).collect()
}

/// The `terminal_update` of each running process, which a resume sends
/// before its snapshot lists them.
pub(super) fn announcements(processes: &[SessionProcess]) -> Vec<v2::SessionUpdate> {
    processes
        .iter()
        .filter(|process| running(process))
        .map(announcement)
        .collect()
}

fn announcement(process: &SessionProcess) -> v2::SessionUpdate {
    v2::SessionUpdate::TerminalUpdate(
        v2::TerminalUpdate::new(&*process.id).command(process.command.clone()),
    )
}

fn exit(id: &str, exit_code: Option<i32>) -> v2::SessionUpdate {
    v2::SessionUpdate::TerminalUpdate(
        v2::TerminalUpdate::new(id)
            .exit_status(v2::TerminalExitStatus::new().exit_code(exit_code.map(i64::from))),
    )
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
