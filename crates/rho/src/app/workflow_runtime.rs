//! Durable execution of already-frozen workflow graphs.
//!
//! Planning and agent discovery do not occur in this module. CLI, tool, and
//! TUI owners can use [`WorkflowRunner`] and consume typed [`RuntimeEvent`]s.

#[path = "workflow_runtime/agent.rs"]
mod agent;
#[path = "workflow_runtime/artifacts.rs"]
mod artifacts;
#[path = "workflow_runtime/cancellation.rs"]
mod cancellation;
#[path = "workflow_runtime/checkout_gate.rs"]
mod checkout_gate;
#[path = "workflow_runtime/command.rs"]
mod command;
#[path = "workflow_runtime/drive_session.rs"]
mod drive_session;
#[path = "workflow_runtime/journal.rs"]
mod journal;
#[path = "workflow_runtime/recovery.rs"]
mod recovery;
#[path = "workflow_runtime/runner.rs"]
mod runner;
#[path = "workflow_runtime/types.rs"]
mod types;

pub(crate) use agent::WorkflowAgentExecutor;
pub(crate) use cancellation::{
    cancellation_request_acknowledged, cross_process_cancel_acknowledged,
    request_cross_process_cancel,
};
pub(crate) use checkout_gate::CheckoutGate;
pub(crate) use command::{CommandHostFactory, WorkflowCommandExecutor};
pub(crate) use runner::{RecoveryDecision, WorkflowRunner};
pub(crate) use types::{
    CleanupCause, NodeExecutionRequest, NodeExecutionResult, NodeProgressReporter,
    NodeProgressUpdate, RuntimeError, RuntimeEvent, RuntimeSecurity, WorkflowExecutionFuture,
    WorkflowNodeExecutor,
};

#[cfg(test)]
#[path = "workflow_runtime/workflow_runtime_tests.rs"]
mod tests;
