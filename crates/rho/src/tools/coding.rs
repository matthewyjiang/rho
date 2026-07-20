use crate::agent::{AgentCapabilities, ToolCapability};

pub(super) fn sdk_bundle(
    capabilities: &AgentCapabilities,
    max_output_bytes: usize,
) -> super::sdk_registry::StaticToolBundle {
    use rho_tools::CodingToolKind;

    let options = rho_tools::CodingToolOptions::new().max_output_bytes(max_output_bytes);
    let mut tools = Vec::new();
    for (capability, kind) in [
        (ToolCapability::ListDir, CodingToolKind::ListDir),
        (ToolCapability::ReadFile, CodingToolKind::ReadFile),
        (ToolCapability::WriteFile, CodingToolKind::WriteFile),
        (ToolCapability::EditFile, CodingToolKind::EditFile),
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
        tools.push(rho_tools::shell_tool(max_output_bytes));
    }
    super::sdk_registry::StaticToolBundle::new(tools)
}

#[cfg(test)]
#[path = "coding_tests.rs"]
mod tests;
