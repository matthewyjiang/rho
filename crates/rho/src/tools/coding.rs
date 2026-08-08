use std::sync::Arc;

use crate::agent::{AgentCapabilities, ToolCapability};
use rho_sdk::ProcessEnvironment;

pub(super) fn sdk_bundle(
    capabilities: &AgentCapabilities,
    max_output_bytes: usize,
    config_edit_tool: crate::config::EditTool,
    process_environment: ProcessEnvironment,
    mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
) -> super::sdk_registry::StaticToolBundle {
    use rho_tools::CodingToolKind;

    let options = rho_tools::CodingToolOptions::new()
        .max_output_bytes(max_output_bytes)
        .edit_tool(config_edit_tool)
        .mutation_observer(Arc::clone(&mutation_observer));
    let mut tools = Vec::new();
    for (capability, kind) in [
        (ToolCapability::ListDir, CodingToolKind::ListDir),
        (ToolCapability::ReadFile, CodingToolKind::ReadFile),
        (ToolCapability::WriteFile, CodingToolKind::WriteFile),
        (ToolCapability::Edit, CodingToolKind::Edit),
        (ToolCapability::Grep, CodingToolKind::Grep),
        (ToolCapability::Glob, CodingToolKind::Glob),
    ] {
        if capabilities.contains(&capability) {
            tools.push(rho_tools::coding_tool(kind, options.clone()));
        }
    }
    #[cfg(unix)]
    let shell_enabled = capabilities.contains(&ToolCapability::Bash);
    #[cfg(windows)]
    let shell_enabled = capabilities.contains(&ToolCapability::Powershell);
    #[cfg(not(any(unix, windows)))]
    let shell_enabled = false;
    if shell_enabled {
        // RTK stays disabled here. Authorization and execution must use the same
        // immutable process description.
        tools.push(rho_tools::shell_tool(
            rho_tools::ShellToolOptions::new()
                .max_output_bytes(max_output_bytes)
                .environment(process_environment)
                .mutation_observer(mutation_observer),
        ));
    }
    super::sdk_registry::StaticToolBundle::new(tools)
}
