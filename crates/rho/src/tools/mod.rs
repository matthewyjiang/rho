pub mod advisor;
pub mod agent;
mod agent_output;
mod coding;
pub(crate) mod process;
pub mod rho;
mod sdk_features;
pub mod sdk_registry;
pub mod skill;
#[cfg(debug_assertions)]
pub(crate) mod tui_fixture;
pub mod web;
pub(crate) mod workflow;
pub(crate) mod workflow_tracker;

/// Stable union of model-facing and host-only built-in tool names.
///
/// Capability filtering may disable a model-facing entry for a particular run.
/// Host-only entries are never sent to a model, but public contracts such as
/// hook matchers still validate against this owning registry.
pub(crate) const CANONICAL_TOOL_NAMES: &[&str] = &[
    "advisor",
    "agent",
    "agents",
    "bash",
    "edit",
    "fetch_content",
    "get_search_content",
    "glob",
    "grep",
    "list_dir",
    "powershell",
    "process",
    "questionnaire",
    "read_file",
    "rho",
    "skill",
    "web_search",
    "workflow",
    "workflow_command",
    "write",
];

/// Returns whether a canonical built-in tool can mutate workspace or run state.
pub(crate) fn canonical_tool_is_mutating(name: &str) -> Option<bool> {
    match name {
        "agent" | "agents" | "bash" | "edit" | "powershell" | "process" | "rho" | "workflow"
        | "workflow_command" | "write" => Some(true),
        "advisor" | "fetch_content" | "get_search_content" | "glob" | "grep" | "list_dir"
        | "questionnaire" | "read_file" | "skill" | "web_search" => Some(false),
        _ => None,
    }
}

/// Built-ins registered only on provider-free host tool registries.
#[cfg(test)]
pub(crate) const HOST_ONLY_TOOL_NAMES: &[&str] = &["workflow_command"];

#[cfg(test)]
#[path = "app_owned_opt_in_tests.rs"]
mod app_owned_opt_in_tests;
