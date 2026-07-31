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

/// Stable superset of tool names exposed by the application's tool registry.
///
/// Capability filtering may disable an entry for a particular run, but public
/// contracts such as hook matchers validate against this owning registry.
pub(crate) const CANONICAL_TOOL_NAMES: &[&str] = &[
    "agent",
    "agents",
    "apply_patch",
    "bash",
    "edit_file",
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
    "write_file",
];

#[cfg(test)]
#[path = "app_owned_opt_in_tests.rs"]
mod app_owned_opt_in_tests;
