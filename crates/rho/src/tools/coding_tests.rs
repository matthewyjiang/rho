use crate::{
    agent::{AgentCapabilities, ToolCapability},
    tools::sdk_registry::ToolBundle,
};

fn tool_names(capability: ToolCapability) -> Vec<String> {
    let mut capabilities = AgentCapabilities::default();
    capabilities.insert(capability);
    super::sdk_bundle(&capabilities, 1_000)
        .tools()
        .iter()
        .map(|tool| tool.spec().name)
        .collect()
}

#[cfg(unix)]
#[test]
fn unix_registers_shell_only_for_bash_capability() {
    assert_eq!(tool_names(ToolCapability::Bash), ["bash"]);
    assert!(tool_names(ToolCapability::Powershell).is_empty());
}

#[cfg(windows)]
#[test]
fn windows_registers_shell_only_for_powershell_capability() {
    assert_eq!(tool_names(ToolCapability::Powershell), ["powershell"]);
    assert!(tool_names(ToolCapability::Bash).is_empty());
}
