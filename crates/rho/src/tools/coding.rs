use std::sync::Arc;

use crate::agent::{AgentCapabilities, ToolCapability};
use rho_sdk::{tool::Tool, ProcessEnvironment};

/// Build the mid-session/startup edit tool with one shared options policy.
pub(super) fn edit_tool(
    edit_format: rho_tools::EditFormat,
    max_output_bytes: usize,
    mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
) -> Arc<dyn Tool> {
    rho_tools::coding_tool(
        rho_tools::CodingToolKind::Edit,
        rho_tools::CodingToolOptions::new()
            .max_output_bytes(max_output_bytes)
            .edit_tool(edit_format)
            .mutation_observer(mutation_observer),
    )
}

pub(super) fn sdk_bundle(
    capabilities: &AgentCapabilities,
    max_output_bytes: usize,
    config_edit_tool: rho_tools::EditFormat,
    process_environment: ProcessEnvironment,
    mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver>,
    file_view: rho_tools::FileViewPolicy,
) -> super::sdk_registry::StaticToolBundle {
    use rho_tools::CodingToolKind;

    file_view.set(config_edit_tool);
    let options = rho_tools::CodingToolOptions::new()
        .max_output_bytes(max_output_bytes)
        .edit_tool(config_edit_tool)
        .file_view(file_view.clone())
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
        if !capabilities.contains(&capability) {
            continue;
        }
        match kind {
            CodingToolKind::Edit => tools.push(edit_tool(
                config_edit_tool,
                max_output_bytes,
                Arc::clone(&mutation_observer),
            )),
            kind => tools.push(rho_tools::coding_tool(kind, options.clone())),
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
