//! Deterministic workflow planning, graph policy, and durable data primitives.

mod canonical;
mod condition;
mod error;
mod ids;
mod layout;
mod migration;
mod model;
mod normalization;
mod planning_limits;
mod scheduler;
mod schema;
mod secure_fs;
mod service;
mod starlark;
mod starlark_api;
mod starlark_diagnostics;
mod starlark_loader;
mod store;
mod transition;
mod validation;
mod value;
mod wire;

pub(crate) use canonical::graph_digest;
pub(crate) use condition::{evaluate_condition, ConditionContext};
pub(crate) use error::{WorkflowError, WorkflowResult};
pub(crate) use ids::*;
pub(crate) use layout::WorkflowLayout;
pub(crate) use migration::check_schema_version;
pub(crate) use model::*;
pub(crate) use normalization::normalize_workflow;
pub(crate) use planning_limits::{
    Budget, FrozenRuntimeLimits, PlanningLimits, PlanningMeasurements,
};
pub(crate) use scheduler::{apply_event, next_actions};
pub(crate) use schema::*;
pub(crate) use secure_fs::{
    ensure_directory_beneath, freeze_directory_identity, freeze_executable_identity,
    open_file_beneath, read_source_beneath, verified_handle_path, verify_directory_identity,
    verify_executable_identity, write_file_beneath, VerifiedExecutable,
};
pub(crate) use service::{FreezePlan, WorkflowService};
pub(crate) use starlark::StarlarkPlanner;
pub(crate) use starlark_diagnostics::Diagnostic;
pub(crate) use starlark_loader::{CollectedSources, SourceCollector};
pub(crate) use store::{RunMutationGuard, WorkflowStore};
pub(crate) use transition::{
    derive_workflow_outcome, validate_lifecycle_transition, validate_reset_transition,
    validate_transition,
};
pub(crate) use validation::{validate_runtime_budgets, validate_workflow};
pub(crate) use value::WorkflowValue;
pub(crate) use wire::*;

#[cfg(test)]
pub(crate) mod test_support;
