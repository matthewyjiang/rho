pub mod advisor;
pub mod agent;
mod agent_output;
mod coding;
pub(crate) mod mcp;
pub(crate) mod process;
pub mod rho;
mod sdk_features;
pub mod sdk_registry;
pub mod skill;
#[cfg(debug_assertions)]
pub(crate) mod tui_fixture;
pub mod web;
pub(crate) mod workflow;
mod workflow_output;
pub(crate) mod workflow_tracker;

/// Returns the stable union of model-facing and host-only built-in tool names.
///
/// Capability filtering may disable a model-facing entry for a particular run.
/// Host-only entries are never sent to a model, but public contracts such as
/// hook matchers still validate against this owning registry.
pub(crate) fn canonical_tool_names() -> &'static [&'static str] {
    static NAMES: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        let mut names = vec![
            "advisor",
            "agent",
            "agents",
            "bash",
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
        names.extend(
            rho_tools::EditFormat::ALL
                .iter()
                .copied()
                .map(rho_tools::EditFormat::tool_name),
        );
        names.sort_unstable();
        names.dedup();
        names
    });
    NAMES.as_slice()
}

/// Returns whether a canonical built-in tool can mutate workspace or run state.
pub(crate) fn canonical_tool_is_mutating(name: &str) -> Option<bool> {
    match name {
        "agent" | "agents" | "bash" | "powershell" | "process" | "rho" | "workflow"
        | "workflow_command" | "write" => Some(true),
        name if rho_tools::EditFormat::is_edit_tool_name(name) => Some(true),
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
