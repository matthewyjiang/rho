use rho_sdk::{
    tool::{ToolContext, ToolError, ToolErrorKind, ToolSecurity},
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget, ProcessEnvironment,
    ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};

use super::sdk_support::{required_string, workspace};

pub fn security_for(name: &str) -> ToolSecurity {
    let capabilities = match name {
        "process" => vec![CapabilityKind::Process],
        "web_search" => vec![CapabilityKind::Network],
        "skill" => vec![CapabilityKind::Skill],
        _ => Vec::new(),
    };
    ToolSecurity::built_in(capabilities)
}

pub async fn authorize_builtin(
    name: &str,
    arguments: &serde_json::Value,
    context: &ToolContext,
    max_output_bytes: usize,
) -> Result<(), ToolError> {
    let source = CapabilitySource::built_in_tool(name);
    match name {
        "process"
            if arguments.get("action").and_then(serde_json::Value::as_str) == Some("start") =>
        {
            let command = required_string(arguments, "command")?;
            authorize_process(
                context,
                ProcessInvocation::shell_from_path(shell_executable(), shell_arguments(), command),
                optional_timeout(arguments)?,
                max_output_bytes,
                source,
            )
            .await
        }
        "web_search" => {
            authorize_request(
                context,
                CapabilityRequest::network(NetworkTarget::ToolManaged, source),
            )
            .await
        }
        "skill" => {
            let name = required_string(arguments, "name")?;
            authorize_request(context, CapabilityRequest::skill(name, None, source)).await
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
        ProcessEnvironment::InheritAll,
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
fn shell_executable() -> &'static str {
    "bash"
}

#[cfg(windows)]
fn shell_executable() -> &'static str {
    "powershell.exe"
}

#[cfg(unix)]
fn shell_arguments() -> Vec<String> {
    vec!["-lc".into()]
}

#[cfg(windows)]
fn shell_arguments() -> Vec<String> {
    vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-Command".into(),
    ]
}
