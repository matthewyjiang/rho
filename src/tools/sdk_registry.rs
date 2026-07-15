//! Application-owned SDK tool construction for headless automation.
//!
//! Workspace coding tools use their dedicated SDK adapters. The remaining
//! built-ins retain their application implementations behind a compatibility
//! adapter until each tool has a dedicated public-contract implementation.
//! Process-manager ownership stays here so automation can clean up background
//! children independently of the SDK runtime lifecycle.

use std::sync::Arc;

use rho_sdk::tool::{
    OperationKind, Tool as SdkTool, ToolContext as SdkToolContext, ToolError as SdkToolError,
    ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata, ToolOutput,
};

use crate::{
    config::Config,
    diagnostics::RuntimeDiagnostics,
    tool::{Tool as AppTool, ToolContext as AppToolContext, ToolError as AppToolError},
};

use super::{
    process::{Process, ProcessLimits, ProcessManager},
    sdk_adapter::{coding_tools, CodingToolOptions},
};

pub struct AutomationToolSet {
    tools: Vec<Arc<dyn SdkTool>>,
    processes: Option<ProcessManager>,
}

impl AutomationToolSet {
    pub fn disabled() -> Self {
        Self {
            tools: Vec::new(),
            processes: None,
        }
    }

    pub fn enabled(config: &Config, diagnostics: RuntimeDiagnostics) -> Self {
        let mut tools =
            coding_tools(CodingToolOptions::new().max_output_bytes(config.max_output_bytes));
        let processes = ProcessManager::new(ProcessLimits::default());
        tools.push(adapt(
            Process::new(processes.clone()),
            config.max_output_bytes,
        ));

        let rtk_enabled = config.rtk && super::rtk::is_available();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        tools.push(adapt(
            super::bash::Bash::new(rtk_enabled),
            config.max_output_bytes,
        ));
        #[cfg(windows)]
        tools.push(adapt(
            super::powershell::PowerShell::new(rtk_enabled),
            config.max_output_bytes,
        ));
        tools.push(adapt(super::skill::Skill, config.max_output_bytes));
        tools.push(adapt(
            super::rho::Rho::new(diagnostics),
            config.max_output_bytes,
        ));

        let (web_search, fetch_content) = super::web::access_tools(config);
        if web_search.is_available() {
            tools.push(adapt(web_search, config.max_output_bytes));
        }
        tools.push(adapt(fetch_content, config.max_output_bytes));
        tools.push(adapt(super::web::GetSearchContent, config.max_output_bytes));

        Self {
            tools,
            processes: Some(processes),
        }
    }

    pub fn tools(&self) -> &[Arc<dyn SdkTool>] {
        &self.tools
    }

    pub fn specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        self.tools.iter().map(|tool| tool.spec()).collect()
    }

    pub async fn shutdown(&self) {
        if let Some(processes) = &self.processes {
            processes.shutdown().await;
        }
    }
}

fn adapt<T>(tool: T, max_output_bytes: usize) -> Arc<dyn SdkTool>
where
    T: AppTool + 'static,
{
    Arc::new(ApplicationToolAdapter {
        inner: tool,
        max_output_bytes,
    })
}

struct ApplicationToolAdapter<T> {
    inner: T,
    max_output_bytes: usize,
}

impl<T> SdkTool for ApplicationToolAdapter<T>
where
    T: AppTool + 'static,
{
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        self.inner.spec()
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            if context.cancellation().is_cancelled() {
                return Err(SdkToolError::cancelled());
            }
            let cwd = context.workspace_root().ok_or_else(|| {
                SdkToolError::new(
                    ToolErrorKind::Execution,
                    "workspace is required for built-in tools",
                )
            })?;
            let id = invocation.id().to_string();
            let arguments = invocation.arguments().clone();
            let app_context = AppToolContext {
                cwd: cwd.to_path_buf(),
                max_output_bytes: self.max_output_bytes,
            };
            let mut ignore_updates = |_| {};
            let result = self
                .inner
                .call_with_updates_and_cancellation(
                    arguments,
                    app_context,
                    id,
                    context.cancellation().clone(),
                    &mut ignore_updates,
                )
                .await
                .map_err(map_app_error)?;
            if !result.ok {
                return Err(SdkToolError::new(ToolErrorKind::Execution, result.content));
            }
            Ok(ToolOutput::text(result.content).metadata(metadata_for(&self.inner.spec().name)))
        })
    }
}

fn metadata_for(name: &str) -> ToolMetadata {
    let operation = match name {
        "bash" | "powershell" | "process" => OperationKind::Execute,
        "web_search" | "fetch_content" | "get_search_content" => OperationKind::Network,
        _ => OperationKind::Other(name.to_string()),
    };
    ToolMetadata::new().operation(operation)
}

fn map_app_error(error: AppToolError) -> SdkToolError {
    let kind = match error {
        AppToolError::InvalidArguments(_) => ToolErrorKind::InvalidArguments,
        AppToolError::Io(_) | AppToolError::Utf8(_) | AppToolError::Message(_) => {
            ToolErrorKind::Execution
        }
    };
    SdkToolError::new(kind, error.to_string())
}
