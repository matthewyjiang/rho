use std::{sync::Arc, time::Duration};

use rho_sdk::{
    tool::{
        OperationKind, PreparedToolInvocation, Tool, ToolContext, ToolError, ToolErrorKind,
        ToolFuture, ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext,
        ToolPrepareFuture, ToolProgress, ToolResource, ToolResourceAccess, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, ProcessEnvironment, ProcessExecution,
    ProcessInvocation, ProcessOutputLimits,
};
use rho_tools::tool::{Tool as LegacyTool, ToolContext as LegacyToolContext};

use super::{Process, ProcessArgs};

pub(crate) struct SdkProcess {
    process: Process,
    max_output_bytes: usize,
    environment: ProcessEnvironment,
}

impl SdkProcess {
    pub(crate) fn new(
        process: Process,
        max_output_bytes: usize,
        environment: ProcessEnvironment,
    ) -> Self {
        Self {
            process,
            max_output_bytes,
            environment,
        }
    }

    async fn execute(
        &self,
        args: ProcessArgs,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        match args {
            ProcessArgs::Start {
                command,
                timeout_seconds,
            } => {
                self.execute_start(command, timeout_seconds.map(Duration::from_secs), context)
                    .await
            }
            args => {
                execute_prepared(
                    &self.process,
                    args,
                    context.workspace_root(),
                    self.max_output_bytes,
                    context.cancellation(),
                    context.progress(),
                )
                .await
            }
        }
    }

    async fn execute_start(
        &self,
        command: String,
        timeout: Option<Duration>,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let workspace = context.workspace().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::Execution,
                "process requires a configured workspace",
            )
        })?;
        let cwd = workspace
            .resolve_for_read(workspace.root())
            .map_err(|error| ToolError::new(ToolErrorKind::PolicyDenied, error.to_string()))?;
        let execution = ProcessExecution::new(
            cwd.path(),
            process_invocation(&command),
            self.environment.clone(),
            ProcessOutputLimits::new(self.max_output_bytes, timeout),
        );
        context
            .authorize(CapabilityRequest::process(
                execution.clone(),
                CapabilitySource::built_in_tool("process"),
            ))
            .await
            .map_err(|error| {
                if error.kind() == rho_sdk::AuthorizationDenialKind::Cancelled {
                    ToolError::cancelled()
                } else {
                    ToolError::policy_denied(&error)
                }
            })?;
        workspace
            .revalidate(&cwd)
            .map_err(|error| ToolError::new(ToolErrorKind::PolicyDenied, error.to_string()))?;

        let mut updates = Vec::new();
        let mut collect_update = |lines| updates.push(lines);
        let start = self.process.start_execution(execution);
        let snapshot = tokio::select! {
            result = start => result,
            () = context.cancellation().cancelled() => return Err(ToolError::cancelled()),
        }
        .map_err(|error| ToolError::new(ToolErrorKind::Execution, error))?;
        collect_update(super::display::snapshot_progress_lines(&snapshot));
        for lines in updates {
            if !context
                .progress()
                .send(ToolProgress::message(lines.join("\n")))
                .await
            {
                break;
            }
        }
        let content = serde_json::to_string(&snapshot)
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
        Ok(ToolOutput::text(content).metadata(process_metadata()))
    }
}

impl Tool for SdkProcess {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        self.process.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Process])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        rho_sdk::tool::call_prepared(self, invocation, context)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let args = parse_args(invocation.into_arguments());
        Box::pin(async move {
            let args = args?;
            let metadata = process_metadata();
            match &args {
                ProcessArgs::Start { .. } => Ok(PreparedToolInvocation::exclusive(
                    metadata,
                    move |context| Box::pin(async move { self.execute(args, &context).await }),
                )),
                ProcessArgs::Poll { process_id, .. } => {
                    let access =
                        ToolResourceAccess::shared(ToolResource::managed_process(process_id));
                    Ok(PreparedToolInvocation::resource_aware(
                        [access],
                        [],
                        metadata,
                        move |context| {
                            Box::pin(async move {
                                execute_prepared(
                                    &self.process,
                                    args,
                                    context.workspace_root(),
                                    self.max_output_bytes,
                                    context.cancellation(),
                                    context.progress(),
                                )
                                .await
                            })
                        },
                    ))
                }
                ProcessArgs::Stop { process_id } => {
                    let access =
                        ToolResourceAccess::exclusive(ToolResource::managed_process(process_id));
                    Ok(PreparedToolInvocation::resource_aware(
                        [access],
                        [],
                        metadata,
                        move |context| {
                            Box::pin(async move {
                                execute_prepared(
                                    &self.process,
                                    args,
                                    context.workspace_root(),
                                    self.max_output_bytes,
                                    context.cancellation(),
                                    context.progress(),
                                )
                                .await
                            })
                        },
                    ))
                }
            }
        })
    }
}

fn parse_args(arguments: serde_json::Value) -> Result<ProcessArgs, ToolError> {
    ProcessArgs::parse(arguments)
        .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error.to_string()))
}

async fn execute_prepared(
    process: &Process,
    args: ProcessArgs,
    workspace_root: Option<&std::path::Path>,
    max_output_bytes: usize,
    cancellation: &rho_sdk::CancellationToken,
    progress: &rho_sdk::tool::ToolProgressSender,
) -> Result<ToolOutput, ToolError> {
    let cwd = workspace_root.ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::Execution,
            "process requires a configured workspace",
        )
    })?;
    let mut updates = Vec::new();
    let mut collect_update = |lines| updates.push(lines);
    let execution = process.execute(
        args,
        LegacyToolContext {
            cwd: cwd.to_path_buf(),
            max_output_bytes,
        },
        String::new(),
        &mut collect_update,
    );
    let result = tokio::select! {
        result = execution => result,
        () = cancellation.cancelled() => return Err(ToolError::cancelled()),
    }
    .map_err(map_legacy_error)?;
    for lines in updates {
        if !progress.send(ToolProgress::message(lines.join("\n"))).await {
            break;
        }
    }
    if !result.ok {
        return Err(ToolError::new(ToolErrorKind::Execution, result.content));
    }
    Ok(ToolOutput::text(result.content).metadata(process_metadata()))
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
        rho_tools::powershell::wrapped_command(command),
    )
}

fn process_metadata() -> ToolMetadata {
    ToolMetadata::new().operation(OperationKind::Execute)
}

fn map_legacy_error(error: rho_tools::tool::ToolError) -> ToolError {
    match error {
        rho_tools::tool::ToolError::InvalidArguments(error) => {
            ToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
        }
        rho_tools::tool::ToolError::Message(message) if message == "tool interrupted" => {
            ToolError::cancelled()
        }
        error => ToolError::new(ToolErrorKind::Execution, error.to_string()),
    }
}

pub(super) fn tool(
    process: Process,
    max_output_bytes: usize,
    environment: ProcessEnvironment,
) -> Arc<dyn rho_sdk::tool::Tool> {
    Arc::new(SdkProcess::new(process, max_output_bytes, environment))
}
