use rho_sdk::{
    tool::{ToolContext, ToolError, ToolErrorKind, ToolSecurity},
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget, ProcessEnvironment,
    ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};

use super::{
    legacy_sdk_adapter::LegacyToolProfile,
    sdk_support::{required_string, workspace},
};

pub(crate) fn legacy_security_for(profile: LegacyToolProfile) -> ToolSecurity {
    let capabilities = match profile {
        LegacyToolProfile::Process => vec![CapabilityKind::Process],
        LegacyToolProfile::WebSearch => vec![CapabilityKind::Network],
        _ => Vec::new(),
    };
    ToolSecurity::built_in(capabilities)
}

pub(crate) async fn authorize_legacy(
    profile: LegacyToolProfile,
    arguments: &serde_json::Value,
    context: &ToolContext,
    max_output_bytes: usize,
) -> Result<(), ToolError> {
    let source = CapabilitySource::built_in_tool(profile.name());
    match profile {
        LegacyToolProfile::Process
            if arguments.get("action").and_then(serde_json::Value::as_str) == Some("start") =>
        {
            let command = required_string(arguments, "command")?;
            authorize_process(
                context,
                process_invocation(command),
                optional_timeout(arguments)?,
                max_output_bytes,
                source,
            )
            .await
        }
        LegacyToolProfile::WebSearch => {
            authorize_request(
                context,
                CapabilityRequest::network(NetworkTarget::ToolManaged, source),
            )
            .await
        }
        _ => Ok(()),
    }
}

pub async fn authorize_request(
    context: &ToolContext,
    request: CapabilityRequest,
) -> Result<(), ToolError> {
    context
        .authorize(request)
        .await
        .map(|_| ())
        .map_err(|error| {
            if error.kind() == rho_sdk::AuthorizationDenialKind::Cancelled {
                ToolError::cancelled()
            } else {
                ToolError::policy_denied(&error)
            }
        })
}

async fn authorize_process(
    context: &ToolContext,
    invocation: ProcessInvocation,
    timeout: Option<std::time::Duration>,
    max_output_bytes: usize,
    source: CapabilitySource,
) -> Result<(), ToolError> {
    let workspace = workspace(context)?;
    let cwd = workspace
        .resolve_for_read(workspace.root())
        .map_err(|error| ToolError::new(ToolErrorKind::PolicyDenied, error.to_string()))?;
    let execution = ProcessExecution::new(
        cwd.path(),
        invocation,
        ProcessEnvironment::inherit_default(),
        ProcessOutputLimits::new(max_output_bytes, timeout),
    );
    authorize_request(context, CapabilityRequest::process(execution, source)).await?;
    workspace
        .revalidate(&cwd)
        .map_err(|error| ToolError::new(ToolErrorKind::PolicyDenied, error.to_string()))
}

fn optional_timeout(
    arguments: &serde_json::Value,
) -> Result<Option<std::time::Duration>, ToolError> {
    arguments
        .get("timeout_seconds")
        .map(|value| {
            value
                .as_u64()
                .filter(|seconds| *seconds > 0)
                .map(std::time::Duration::from_secs)
                .ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "timeout_seconds must be a positive integer",
                    )
                })
        })
        .transpose()
}

#[cfg(unix)]
fn process_invocation(command: &str) -> ProcessInvocation {
    ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command.to_string())
}

#[cfg(windows)]
fn process_invocation(command: &str) -> ProcessInvocation {
    ProcessInvocation::shell_from_path(
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
        ],
        crate::powershell::wrapped_command(command),
    )
}

#[cfg(test)]
#[path = "sdk_security_tests.rs"]
mod tests;
