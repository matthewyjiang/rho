use std::path::PathBuf;

use thiserror::Error;

use super::NodeId;

pub(crate) type WorkflowResult<T> = Result<T, WorkflowError>;

#[derive(Debug, Error)]
pub(crate) enum WorkflowError {
    #[error("invalid {kind} '{value}': expected {grammar}")]
    InvalidId {
        kind: &'static str,
        value: String,
        grammar: &'static str,
    },
    #[error("{budget} budget exceeded: accepted limit {limit}, requested or measured {actual}")]
    BudgetExceeded {
        budget: &'static str,
        limit: u64,
        actual: u64,
    },
    #[error("workflow graph contains a cycle through node '{node}'")]
    Cycle { node: NodeId },
    #[error("node '{node}' needs unknown node '{dependency}'")]
    MissingDependency { node: NodeId, dependency: NodeId },
    #[error("node '{node}' has a reference to non-ancestor node '{referenced}'")]
    NonAncestorReference { node: NodeId, referenced: NodeId },
    #[error("node '{node}' has invalid workspace access: {reason}")]
    InvalidAccess { node: NodeId, reason: String },
    #[error("schema error at {path}: {reason}")]
    Schema { path: String, reason: String },
    #[error("condition error: {0}")]
    Condition(String),
    #[error("illegal node transition for '{node}': {from} -> {to}")]
    IllegalTransition {
        node: NodeId,
        from: String,
        to: String,
    },
    #[error("scheduler state does not match graph: {0}")]
    Scheduler(String),
    #[error("invalid Starlark module label '{label}': {reason}")]
    InvalidModuleLabel { label: String, reason: String },
    #[error("workflow source path is outside module root: {path}")]
    SourceOutsideRoot { path: PathBuf },
    #[error("workflow source path contains a symlink: {path}")]
    SourceSymlink { path: PathBuf },
    #[error("workflow import cycle: {chain}")]
    ImportCycle { chain: String },
    #[error("entry module must export exactly one WORKFLOW definition")]
    MissingWorkflow,
    #[error("unsupported workflow value at {path}: {kind}")]
    UnsupportedValue { path: String, kind: String },
    #[error("missing required workflow input '{0}'")]
    MissingInput(String),
    #[error("unknown workflow input '{0}'")]
    UnknownInput(String),
    #[error("workflow input '{name}' is invalid: {reason}")]
    InvalidInput { name: String, reason: String },
    #[error("workflow data is corrupt at {path}: {reason}")]
    Corrupt { path: PathBuf, reason: String },
    #[error("unsupported {kind} schema version {found}; supported version is {supported}")]
    UnsupportedVersion {
        kind: &'static str,
        found: u32,
        supported: u32,
    },
    #[error("workflow ID prefix '{prefix}' is ambiguous: {matches} matches")]
    AmbiguousId { prefix: String, matches: usize },
    #[error("unknown workflow ID '{0}'")]
    UnknownId(String),
    #[error("workflow store boundary is not a trusted directory: {0}")]
    UntrustedDirectory(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Starlark evaluation failed: {0}")]
    Starlark(String),
}
