use rho_sdk::{
    tool::{
        OperationKind, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation,
        ToolMetadata, ToolOutput, ToolProgress, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, ProcessEnvironment, ProcessExecution,
    ProcessInvocation, ProcessOutputLimits, ResolvedWorkspacePath,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    cancellation::RunCancellation,
    tool::{Tool as AppTool, ToolError as AppToolError, ToolResult as AppToolResult},
    DEFAULT_MAX_OUTPUT_BYTES,
};

use super::{sdk_security::authorize_request, sdk_support::check_cancelled};

/// Options for the host-facing shell tool adapter.
#[derive(Clone, Debug)]
pub struct ShellToolOptions {
    max_output_bytes: usize,
    environment: ProcessEnvironment,
}

impl Default for ShellToolOptions {
    fn default() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            environment: ProcessEnvironment::InheritAll,
        }
    }
}

impl ShellToolOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes.max(1);
        self
    }

    pub fn environment(mut self, environment: ProcessEnvironment) -> Self {
        self.environment = environment;
        self
    }
}

#[derive(Clone, Copy)]
enum ShellKind {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Bash,
    #[cfg(windows)]
    PowerShell,
}

struct SdkShellTool {
    kind: ShellKind,
    max_output_bytes: usize,
    environment: ProcessEnvironment,
}

impl SdkShellTool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn bash(options: ShellToolOptions) -> Self {
        Self {
            kind: ShellKind::Bash,
            max_output_bytes: options.max_output_bytes,
            environment: options.environment,
        }
    }

    #[cfg(windows)]
    fn powershell(options: ShellToolOptions) -> Self {
        Self {
            kind: ShellKind::PowerShell,
            max_output_bytes: options.max_output_bytes,
            environment: options.environment,
        }
    }
}

#[derive(Deserialize)]
struct ShellArgs {
    command: String,
    timeout_seconds: Option<u64>,
}

struct ShellPlan {
    execution: ProcessExecution,
    resolved_cwd: ResolvedWorkspacePath,
}

impl ShellPlan {
    fn parse(
        kind: ShellKind,
        arguments: Value,
        context: &ToolContext,
        max_output_bytes: usize,
        environment: ProcessEnvironment,
    ) -> Result<Self, ToolError> {
        let arguments: ShellArgs = serde_json::from_value(arguments).map_err(|error| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                format!("invalid shell arguments: {error}"),
            )
        })?;
        let timeout = arguments
            .timeout_seconds
            .map(|seconds| {
                if seconds == 0 {
                    Err(ToolError::new(
                        ToolErrorKind::InvalidArguments,
                        "timeout_seconds must be greater than zero",
                    ))
                } else {
                    Ok(std::time::Duration::from_secs(seconds))
                }
            })
            .transpose()?;
        let workspace = context.workspace().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::Execution,
                "workspace is required for shell tools",
            )
        })?;
        let resolved_cwd = workspace
            .resolve_for_read(workspace.root())
            .map_err(|error| ToolError::new(ToolErrorKind::PolicyDenied, error.to_string()))?;
        let execution = ProcessExecution::new(
            resolved_cwd.path(),
            kind.invocation(arguments.command),
            environment,
            ProcessOutputLimits::new(max_output_bytes, timeout),
        );
        Ok(Self {
            execution,
            resolved_cwd,
        })
    }

    async fn authorize(&self, kind: ShellKind, context: &ToolContext) -> Result<(), ToolError> {
        authorize_request(
            context,
            CapabilityRequest::process(
                self.execution.clone(),
                CapabilitySource::built_in_tool(kind.name()),
            ),
        )
        .await
    }

    async fn execute(
        self,
        kind: ShellKind,
        invocation_id: String,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let workspace = context.workspace().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::Execution,
                "workspace is required for shell tools",
            )
        })?;
        workspace
            .revalidate(&self.resolved_cwd)
            .map_err(|error| ToolError::new(ToolErrorKind::PolicyDenied, error.to_string()))?;
        let result = execute_with_progress(kind, self.execution, invocation_id, context).await?;
        if !result.ok {
            return Err(ToolError::new(ToolErrorKind::Execution, result.content));
        }
        Ok(ToolOutput::text(result.content)
            .metadata(ToolMetadata::new().operation(OperationKind::Execute)))
    }
}

impl ShellKind {
    const fn name(self) -> &'static str {
        match self {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Self::Bash => "bash",
            #[cfg(windows)]
            Self::PowerShell => "powershell",
        }
    }

    fn invocation(self, command: String) -> ProcessInvocation {
        match self {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Self::Bash => ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command),
            #[cfg(windows)]
            Self::PowerShell => ProcessInvocation::shell_from_path(
                "powershell.exe",
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                ],
                super::powershell::wrapped_command(&command),
            ),
        }
    }

    async fn execute(
        self,
        execution: ProcessExecution,
        invocation_id: String,
        cancellation: RunCancellation,
        on_update: &mut (dyn FnMut(Vec<String>) + Send),
    ) -> Result<AppToolResult, AppToolError> {
        match self {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Self::Bash => {
                super::bash::execute_process(execution, invocation_id, cancellation, on_update)
                    .await
            }
            #[cfg(windows)]
            Self::PowerShell => {
                super::powershell::execute_process(
                    execution,
                    invocation_id,
                    cancellation,
                    on_update,
                )
                .await
            }
        }
    }
}

impl Tool for SdkShellTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        match self.kind {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            ShellKind::Bash => super::bash::Bash::new(/*rtk_enabled*/ false).spec(),
            #[cfg(windows)]
            ShellKind::PowerShell => {
                super::powershell::PowerShell::new(/*rtk_enabled*/ false).spec()
            }
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Process])
    }

    fn start_metadata(&self, _arguments: &Value) -> ToolMetadata {
        ToolMetadata::new().operation(OperationKind::Execute)
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let invocation_id = invocation.id().to_string();
            let plan = ShellPlan::parse(
                self.kind,
                invocation.into_arguments(),
                &context,
                self.max_output_bytes,
                self.environment.clone(),
            )?;
            plan.authorize(self.kind, &context).await?;
            plan.execute(self.kind, invocation_id, &context).await
        })
    }
}

async fn execute_with_progress(
    kind: ShellKind,
    execution: ProcessExecution,
    invocation_id: String,
    context: &ToolContext,
) -> Result<AppToolResult, ToolError> {
    let (update_sender, mut updates) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
    let mut on_update = move |lines: Vec<String>| {
        let _ = update_sender.send(lines);
    };
    let call = kind.execute(
        execution,
        invocation_id,
        context.cancellation().clone(),
        &mut on_update,
    );
    tokio::pin!(call);
    let mut updates_open = true;
    let result = loop {
        tokio::select! {
            result = &mut call => break result,
            update = updates.recv(), if updates_open => {
                match update {
                    Some(lines) => {
                        let _ = context
                            .progress()
                            .send(ToolProgress::message(lines.join("\n")))
                            .await;
                    }
                    None => updates_open = false,
                }
            }
        }
    };
    while let Ok(lines) = updates.try_recv() {
        let _ = context
            .progress()
            .send(ToolProgress::message(lines.join("\n")))
            .await;
    }
    result.map_err(map_app_error)
}

fn map_app_error(error: AppToolError) -> ToolError {
    match &error {
        AppToolError::InvalidArguments(_) => {
            ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
        }
        AppToolError::Message(message) if message == "tool interrupted" => ToolError::cancelled(),
        AppToolError::Io(_) | AppToolError::Utf8(_) | AppToolError::Message(_) => {
            ToolError::new(ToolErrorKind::Execution, error.to_string())
        }
    }
}

#[cfg(test)]
#[path = "sdk_shell_tests.rs"]
mod tests;

/// Returns the workspace shell tool (`bash` on Linux/macOS, PowerShell on
/// Windows) as an SDK tool trait object.
///
/// The tool does not grant capabilities by itself. Hosts must attach a
/// workspace and a non-default policy on the runtime before commands run.
/// Default environment inheritance is [`ProcessEnvironment::InheritAll`];
/// security-sensitive hosts should pass a stricter policy through
/// [`ShellToolOptions::environment`].
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn shell_tool(options: ShellToolOptions) -> std::sync::Arc<dyn Tool> {
    std::sync::Arc::new(SdkShellTool::bash(options))
}

/// Returns the workspace shell tool (`bash` on Linux/macOS, PowerShell on
/// Windows) as an SDK tool trait object.
///
/// The tool does not grant capabilities by itself. Hosts must attach a
/// workspace and a non-default policy on the runtime before commands run.
/// Default environment inheritance is [`ProcessEnvironment::InheritAll`];
/// security-sensitive hosts should pass a stricter policy through
/// [`ShellToolOptions::environment`].
#[cfg(windows)]
pub fn shell_tool(options: ShellToolOptions) -> std::sync::Arc<dyn Tool> {
    std::sync::Arc::new(SdkShellTool::powershell(options))
}
