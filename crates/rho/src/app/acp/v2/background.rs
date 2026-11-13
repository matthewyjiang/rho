//! Rho's background work as ACP v2 background work.
//!
//! Each piece maps to the entity v2 lists it as:
//!
//! - a process the `process` tool started is a terminal, named by the
//!   process id;
//! - a workflow run is a tool call Rho starts itself (`rho-workflow-<run>`),
//!   since the `workflow` tool call that launched it already finished;
//! - a subagent is a child session, named by its own session id, or by its
//!   run id when its runtime keeps no Rho session.
//!
//! Each is announced (`terminal_update`, `tool_call_update`,
//! `subagent_update`) before the first `state_update` that lists it, with
//! the capability to cancel it, and finalized (exit status, final status,
//! idle) before the first one that no longer does. Rho reports background
//! work on every `state_update`, so each one carries the list, empty when
//! nothing runs.
//!
//! The daemon keeps listing work that ended until its model is given the
//! result; such work no longer runs, so it is finalized and left out here.

use agent_client_protocol::{schema::MaybeUndefined, v2::schema as v2};
use rho_protocol::{BackgroundItem, BackgroundStatus, BackgroundWork};

use crate::app::acp::Foreground;

/// The prefix of the tool calls Rho starts for its workflow runs.
const WORKFLOW_TOOL_CALL: &str = "rho-workflow-";

/// What v2 names a piece of background work by.
#[derive(Debug, PartialEq, Eq)]
enum Entity {
    Terminal(String),
    ToolCall(String),
    Subagent(String),
}

impl Entity {
    /// The entity of `item`, if v2 has one for it.
    fn of(item: &BackgroundItem) -> Option<Self> {
        match item {
            BackgroundItem::Process { id } => Some(Self::Terminal(id.clone())),
            BackgroundItem::Workflow { run } => {
                Some(Self::ToolCall(format!("{WORKFLOW_TOOL_CALL}{run}")))
            }
            BackgroundItem::Subagent { run, session } => Some(Self::Subagent(
                session
                    .as_ref()
                    .map_or_else(|| run.clone(), |session| session.as_str().to_owned()),
            )),
            // An item a newer daemon sends; this rho cannot name it.
            _ => None,
        }
    }

    fn listed(&self) -> v2::BackgroundWork {
        match self {
            Self::Terminal(id) => v2::BackgroundWork::Terminal(v2::BackgroundTerminal::new(&**id)),
            Self::ToolCall(id) => v2::BackgroundWork::ToolCall(v2::BackgroundToolCall::new(&**id)),
            Self::Subagent(id) => v2::BackgroundWork::Subagent(v2::BackgroundSubagent::new(&**id)),
        }
    }
}

/// The `state_update` of `foreground`, listing the running `work`, for a
/// state that repeats or changes within a turn: no stop reason.
pub(super) fn state(foreground: Foreground, work: &[BackgroundWork]) -> v2::SessionUpdate {
    let listed = listed(work);
    v2::SessionUpdate::StateUpdate(match foreground {
        Foreground::Running => {
            v2::StateUpdate::Running(v2::RunningStateUpdate::new().background(listed))
        }
        Foreground::RequiresAction => {
            v2::StateUpdate::RequiresAction(v2::RequiresActionStateUpdate::new().background(listed))
        }
        Foreground::Idle => v2::StateUpdate::Idle(v2::IdleStateUpdate::new().background(listed)),
    })
}

/// `idle`, with why foreground work stopped, listing the running `work`.
pub(super) fn idle_state(
    stop_reason: Option<v2::StopReason>,
    work: &[BackgroundWork],
) -> v2::SessionUpdate {
    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(
        v2::IdleStateUpdate::new()
            .stop_reason(stop_reason)
            .background(listed(work)),
    ))
}

/// The background list of a `state_update`: the running work.
fn listed(work: &[BackgroundWork]) -> Vec<v2::BackgroundWork> {
    work.iter()
        .filter(|work| work.status.is_running())
        .filter_map(|work| Entity::of(&work.item))
        .map(|entity| entity.listed())
        .collect()
}

/// The updates that bring a host that saw `previous` to `current`: an
/// announcement for each piece that started running, and the final state
/// of each that stopped. One that left the list without being seen to end
/// is finalized as far as the host can tell.
pub(super) fn changes(
    previous: &[BackgroundWork],
    current: &[BackgroundWork],
) -> Vec<v2::SessionUpdate> {
    let was_running = |item: &BackgroundItem| {
        previous
            .iter()
            .any(|work| work.item == *item && work.status.is_running())
    };
    let started = current
        .iter()
        .filter(|work| work.status.is_running() && !was_running(&work.item))
        .filter_map(announcement);
    let stopped = previous
        .iter()
        .filter(|work| work.status.is_running())
        .filter_map(
            |before| match current.iter().find(|work| work.item == before.item) {
                Some(now) if now.status.is_running() => None,
                Some(now) => finalization(now),
                None => finalization(&BackgroundWork {
                    status: BackgroundStatus::Unknown,
                    exit_code: None,
                    ..before.clone()
                }),
            },
        );
    started.chain(stopped).collect()
}

/// The announcement of each piece of running `work`, which a resume sends
/// before its snapshot lists them.
pub(super) fn announcements(work: &[BackgroundWork]) -> Vec<v2::SessionUpdate> {
    work.iter()
        .filter(|work| work.status.is_running())
        .filter_map(announcement)
        .collect()
}

fn announcement(work: &BackgroundWork) -> Option<v2::SessionUpdate> {
    Some(match Entity::of(&work.item)? {
        Entity::Terminal(id) => v2::SessionUpdate::TerminalUpdate(
            v2::TerminalUpdate::new(id)
                .command(work.title.clone())
                .capabilities(
                    v2::TerminalCapabilities::new().cancel(v2::TerminalCancelCapabilities::new()),
                ),
        ),
        Entity::ToolCall(id) => v2::SessionUpdate::ToolCallUpdate(
            v2::ToolCallUpdate::new(id)
                .title(work.title.clone())
                .kind(v2::ToolKind::Other)
                .status(v2::ToolCallStatus::InProgress)
                .capabilities(
                    v2::ToolCallCapabilities::new().cancel(v2::ToolCallCancelCapabilities::new()),
                ),
        ),
        Entity::Subagent(id) => v2::SessionUpdate::SubagentUpdate(
            v2::SubagentUpdate::new(id)
                .title(work.title.clone())
                .state(v2::StateUpdate::Running(v2::RunningStateUpdate::new()))
                .capabilities(
                    v2::SubagentSessionCapabilities::new()
                        .cancel(v2::SessionCancelCapabilities::new()),
                ),
        ),
    })
}

/// The update that ends `work`, which stopped as its status says, and
/// withdraws the capability to cancel it. A status of
/// [`BackgroundStatus::Unknown`] says the host cannot tell how it ended.
fn finalization(work: &BackgroundWork) -> Option<v2::SessionUpdate> {
    Some(match Entity::of(&work.item)? {
        Entity::Terminal(id) => v2::SessionUpdate::TerminalUpdate(
            v2::TerminalUpdate::new(id)
                .exit_status(v2::TerminalExitStatus::new().exit_code(work.exit_code.map(i64::from)))
                .capabilities(MaybeUndefined::Null),
        ),
        Entity::ToolCall(id) => {
            let status = match work.status {
                BackgroundStatus::Failed => v2::ToolCallStatus::Failed,
                BackgroundStatus::Cancelled => v2::ToolCallStatus::Cancelled,
                _ => v2::ToolCallStatus::Completed,
            };
            v2::SessionUpdate::ToolCallUpdate(
                v2::ToolCallUpdate::new(id)
                    .status(status)
                    .capabilities(MaybeUndefined::Null),
            )
        }
        Entity::Subagent(id) => {
            let stop_reason = match work.status {
                BackgroundStatus::Completed => Some(v2::StopReason::EndTurn),
                BackgroundStatus::Cancelled => Some(v2::StopReason::Cancelled),
                BackgroundStatus::Failed => Some(v2::StopReason::Error(v2::ErrorStopReason::new())),
                _ => None,
            };
            v2::SessionUpdate::SubagentUpdate(
                v2::SubagentUpdate::new(id)
                    .state(v2::StateUpdate::Idle(
                        v2::IdleStateUpdate::new().stop_reason(stop_reason),
                    ))
                    .capabilities(MaybeUndefined::Null),
            )
        }
    })
}

/// The running piece of `work` that terminal `terminal_id` names.
pub(super) fn terminal(work: &[BackgroundWork], terminal_id: &str) -> Option<BackgroundItem> {
    running(work, &Entity::Terminal(terminal_id.to_owned()))
}

/// The running piece of `work` that tool call `tool_call_id` names.
pub(super) fn tool_call(work: &[BackgroundWork], tool_call_id: &str) -> Option<BackgroundItem> {
    running(work, &Entity::ToolCall(tool_call_id.to_owned()))
}

/// The running piece of `work` that child session `session_id` names.
pub(super) fn subagent(work: &[BackgroundWork], session_id: &str) -> Option<BackgroundItem> {
    running(work, &Entity::Subagent(session_id.to_owned()))
}

fn running(work: &[BackgroundWork], entity: &Entity) -> Option<BackgroundItem> {
    work.iter()
        .filter(|work| work.status.is_running())
        .find(|work| Entity::of(&work.item).as_ref() == Some(entity))
        .map(|work| work.item.clone())
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
