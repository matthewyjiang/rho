use std::{path::PathBuf, process::Stdio, time::Duration};

use rho_sdk::{
    tool::{ToolContext, ToolError, ToolErrorKind},
    CapabilityRequest, ExecutableSelection, NetworkTarget, ProcessEnvironment, ProcessExecution,
    ProcessInvocation, ProcessOutputLimits,
};
use tokio::process::Command;

use super::{authorize, capability_source, map_app_tool_error};
use crate::tools::web::{fetch::github, storage};

const GIT_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) struct GitHubClonePlan {
    requested: String,
    target: github::GitHubTarget,
    network_url: String,
    local_path: PathBuf,
    working_directory: PathBuf,
    max_output_bytes: usize,
    processes: Vec<ProcessExecution>,
}

impl GitHubClonePlan {
    pub(super) fn new(
        requested: String,
        target: github::GitHubTarget,
        response_id: &str,
        target_index: usize,
        working_directory: &std::path::Path,
        max_output_bytes: usize,
    ) -> Self {
        let network_url = github::clone_url(&target);
        // Keep each force-cloned target under its own directory so multi-URL
        // fetches with a shared response id cannot collide.
        let local_path = storage::web_access_cache_root()
            .join(std::process::id().to_string())
            .join("github")
            .join(response_id)
            .join(target_index.to_string());
        let processes = process_plan(
            working_directory,
            &network_url,
            &local_path,
            target.ref_name.as_deref(),
            max_output_bytes,
        );
        Self {
            requested,
            target,
            network_url,
            local_path,
            working_directory: working_directory.to_path_buf(),
            max_output_bytes,
            processes,
        }
    }

    pub(super) fn requested(&self) -> &str {
        &self.requested
    }

    pub(super) async fn authorize(&self, context: &ToolContext) -> Result<(), ToolError> {
        authorize(
            context,
            CapabilityRequest::network(
                NetworkTarget::Url(self.network_url.clone()),
                capability_source(),
            ),
        )
        .await?;
        for process in &self.processes {
            authorize(
                context,
                CapabilityRequest::process(process.clone(), capability_source()),
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn execute(self) -> Result<super::FetchedTarget, ToolError> {
        if let Some(parent) = self.local_path.parent() {
            storage::create_private_dir_all(parent).map_err(map_app_tool_error)?;
        }
        // Authorize with the public clone URL, but inject a token-backed URL only
        // when executing so private repos can clone without leaking credentials
        // into capability requests.
        let clone_url = github::authenticated_clone_url(&self.target);
        let processes = process_plan(
            &self.working_directory,
            &clone_url,
            &self.local_path,
            self.target.ref_name.as_deref(),
            self.max_output_bytes,
        );
        for process in &processes {
            run_process(process).await?;
        }
        github::read_clone(&self.target, &self.local_path)
            .await
            .map_err(map_app_tool_error)
    }
}

fn process_plan(
    working_directory: &std::path::Path,
    clone_url: &str,
    local_path: &std::path::Path,
    ref_name: Option<&str>,
    max_output_bytes: usize,
) -> Vec<ProcessExecution> {
    let mut commands = vec![vec![
        "clone".into(),
        "--depth".into(),
        "1".into(),
        clone_url.to_owned(),
        local_path.to_string_lossy().into_owned(),
    ]];
    if let Some(ref_name) = ref_name {
        commands.push(vec![
            "-C".into(),
            local_path.to_string_lossy().into_owned(),
            "fetch".into(),
            "--depth".into(),
            "1".into(),
            "origin".into(),
            ref_name.to_owned(),
        ]);
        commands.push(vec![
            "-C".into(),
            local_path.to_string_lossy().into_owned(),
            "checkout".into(),
            "--detach".into(),
            "FETCH_HEAD".into(),
        ]);
    }
    commands
        .into_iter()
        .map(|arguments| {
            ProcessExecution::new(
                working_directory,
                ProcessInvocation::executable_from_path("git", arguments),
                ProcessEnvironment::inherit_default(),
                ProcessOutputLimits::new(max_output_bytes, Some(GIT_TIMEOUT)),
            )
        })
        .collect()
}

async fn run_process(execution: &ProcessExecution) -> Result<(), ToolError> {
    let ProcessInvocation::Executable {
        executable,
        selection: ExecutableSelection::SearchPath,
        arguments,
    } = execution.invocation()
    else {
        return Err(ToolError::new(
            ToolErrorKind::Execution,
            "fetch_content received an unsupported process plan",
        ));
    };
    if !matches!(
        execution.environment(),
        ProcessEnvironment::InheritAll | ProcessEnvironment::InheritExcept { .. }
    ) {
        return Err(ToolError::new(
            ToolErrorKind::Execution,
            "fetch_content received an unsupported process environment",
        ));
    }
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .current_dir(execution.working_directory())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let ProcessEnvironment::InheritExcept { variable_names } = execution.environment() {
        for name in variable_names {
            command.env_remove(name);
        }
    }
    let timeout = execution.output_limits().timeout().unwrap_or(GIT_TIMEOUT);
    let status = tokio::time::timeout(timeout, command.status())
        .await
        .map_err(|_| ToolError::new(ToolErrorKind::Execution, "git operation timed out"))?
        .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::new(
            ToolErrorKind::Execution,
            format!("git operation failed with status {status}"),
        ))
    }
}
