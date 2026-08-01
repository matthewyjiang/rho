//! Separate terminal mode for observing and controlling a workflow run.
//!
//! This module does not read the workflow store or own runner tasks. The CLI
//! integration must construct an [`event_adapter::WorkflowEventAdapter`] from
//! its run service, pass durable snapshots through it, and apply every action
//! before acknowledging the matching update. This keeps runner policy out of
//! the TUI and lets every error path restore the terminal in one place.
//!
//! The CLI integrator must select this mode only for interactive input and
//! output with no explicit text or JSONL format. In debug matrix mode, match
//! `MATRIX_WORKFLOW_PLAN_ID` or `MATRIX_WORKFLOW_RUN_ID` before store lookup,
//! then call `matrix_adapter` with the matching run or resume start.

mod app;
mod control;
mod dag;
mod event_adapter;
mod input;
pub(crate) mod snapshot;
mod state;
mod view;

#[allow(unused_imports)] // Used by the workflow CLI integration once its service lands.
pub(crate) use app::{run, WorkflowTuiExit};
#[allow(unused_imports)] // This is the TUI-owned integration surface.
pub(crate) use event_adapter::{
    ArtifactKind, ArtifactReference, CancellationState, ExecutionMetadata, PlanApprovalState,
    RecoveryRequirement, SourceDigestSummary, TerminalReason, WorkflowAction, WorkflowEvent,
    WorkflowEventAdapter, WorkflowNodeSnapshot, WorkflowProgress, WorkflowSession,
    WorkflowSnapshot,
};

#[cfg(debug_assertions)]
#[allow(unused_imports)] // Used only by the debug CLI matrix launch path.
pub(crate) use event_adapter::{
    matrix_adapter, MatrixWorkflowStart, MATRIX_WORKFLOW_PLAN_ID, MATRIX_WORKFLOW_RUN_ID,
};
