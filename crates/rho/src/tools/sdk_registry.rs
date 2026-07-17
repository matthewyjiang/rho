//! Application-owned SDK tool construction for headless automation.
//!
//! Workspace coding tools use their dedicated SDK adapters. The remaining
//! built-ins retain their application implementations behind a compatibility
//! adapter until each tool has a dedicated public-contract implementation.
//! Process-manager ownership stays here so automation can clean up background
//! children independently of the SDK runtime lifecycle.

use std::sync::Arc;

use rho_sdk::{
    tool::{
        OperationKind, Tool as SdkTool, ToolContext as SdkToolContext, ToolError as SdkToolError,
        ToolErrorKind, ToolFuture, ToolInvocation, ToolMetadata, ToolOutput, ToolProgress,
        ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, HostChoice, HostInputRequest,
    HostQuestion, SelectionMode,
};

use crate::{
    app::agent_executor::AgentExecutor,
    config::Config,
    diagnostics::RuntimeDiagnostics,
    tool::{truncate, Tool as AppTool, ToolContext as AppToolContext, ToolError as AppToolError},
};

use super::{
    agent::{BackgroundSubagents, SubagentManager},
    process::{Process, ProcessLimits, ProcessManager},
    sdk_adapter::{coding_tools, CodingToolOptions},
    sdk_security::{authorize_builtin, authorize_request, security_for},
    sdk_support::{check_cancelled, required_string, workspace, workspace_root},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct DelegationToolOptions {
    cwd: std::path::PathBuf,
    launch: bool,
    manage: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolSetOptions {
    questionnaire: bool,
    /// Delegation tool selection and its agent discovery working directory.
    delegation: Option<DelegationToolOptions>,
    subagent_config_path: Option<std::path::PathBuf>,
    background_subagents: bool,
}

impl ToolSetOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn questionnaire(mut self, enabled: bool) -> Self {
        self.questionnaire = enabled;
        self
    }

    pub fn delegation_tools(
        mut self,
        cwd: Option<std::path::PathBuf>,
        allowed_tools: &std::collections::BTreeSet<String>,
    ) -> Self {
        self.delegation = cwd.and_then(|cwd| {
            let options = DelegationToolOptions {
                cwd,
                launch: allowed_tools.contains("agent"),
                manage: allowed_tools.contains("agents"),
            };
            (options.launch || options.manage).then_some(options)
        });
        self
    }

    pub fn subagent_config_path(mut self, path: std::path::PathBuf) -> Self {
        self.subagent_config_path = Some(path);
        self
    }

    pub fn background_subagents(mut self, enabled: bool) -> Self {
        self.background_subagents = enabled;
        self
    }
}

pub struct AppToolSet {
    tools: Vec<Arc<dyn SdkTool>>,
    processes: Option<ProcessManager>,
    subagents: Option<SubagentManager>,
}

impl AppToolSet {
    pub fn disabled() -> Self {
        Self {
            tools: Vec::new(),
            processes: None,
            subagents: None,
        }
    }

    pub fn new(config: &Config, diagnostics: RuntimeDiagnostics, options: ToolSetOptions) -> Self {
        let mut tools =
            coding_tools(CodingToolOptions::new().max_output_bytes(config.max_output_bytes));
        let processes = ProcessManager::new(ProcessLimits {
            max_bytes: config.max_output_bytes,
            ..ProcessLimits::default()
        });
        tools.push(adapt(
            Process::new(processes.clone()),
            config.max_output_bytes,
        ));

        // RTK is intentionally disabled for SDK shell tools. Their immutable
        // ProcessExecution must be identical during authorization and execution.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        tools.push(Arc::new(super::sdk_shell::SdkShellTool::bash(
            config.max_output_bytes,
        )));
        #[cfg(windows)]
        tools.push(Arc::new(super::sdk_shell::SdkShellTool::powershell(
            config.max_output_bytes,
        )));
        tools.push(Arc::new(SdkSkillTool {
            max_output_bytes: config.max_output_bytes,
        }));
        tools.push(adapt(
            super::rho::Rho::new(diagnostics),
            config.max_output_bytes,
        ));
        if options.questionnaire {
            tools.push(Arc::new(QuestionnaireTool));
        }
        #[cfg(debug_assertions)]
        if let Some(tool) = super::tui_fixture::from_env() {
            tools.push(tool);
        }

        let web_search = super::web::access_tools(config);
        if web_search.is_available() {
            tools.push(adapt(web_search, config.max_output_bytes));
        }
        tools.push(Arc::new(super::web::SdkFetchContent::new(
            config.max_output_bytes,
        )));
        tools.push(adapt(super::web::GetSearchContent, config.max_output_bytes));

        let subagents = options
            .delegation
            .filter(|_| config.enable_subagents)
            .map(|delegation| {
                let cwd = delegation.cwd;
                let manager = SubagentManager::new(AgentExecutor::new(
                    config.clone(),
                    options.subagent_config_path.clone().unwrap_or_default(),
                    cwd.clone(),
                ));
                if delegation.launch {
                    tools.push(adapt(
                        super::agent::AgentTool::new(
                            manager.clone(),
                            &cwd,
                            if options.background_subagents {
                                BackgroundSubagents::Enabled
                            } else {
                                BackgroundSubagents::Disabled
                            },
                        ),
                        config.max_output_bytes,
                    ));
                }
                if delegation.manage {
                    tools.push(adapt(
                        super::agent::AgentsTool::new(manager.clone()),
                        config.max_output_bytes,
                    ));
                }
                manager
            });

        Self {
            tools,
            processes: Some(processes),
            subagents,
        }
    }

    pub fn tools(&self) -> &[Arc<dyn SdkTool>] {
        &self.tools
    }

    pub fn specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        self.tools.iter().map(|tool| tool.spec()).collect()
    }

    pub fn subagents(&self) -> Option<&SubagentManager> {
        self.subagents.as_ref()
    }

    /// Restricts the set to the named capabilities before model exposure.
    pub fn retain_named(&mut self, names: &[String]) {
        self.tools
            .retain(|tool| names.iter().any(|name| name == &tool.spec().name));
    }

    pub async fn shutdown(&self) {
        if let Some(processes) = &self.processes {
            processes.shutdown().await;
        }
        if let Some(subagents) = &self.subagents {
            subagents.shutdown().await;
        }
    }
}

struct SdkSkillTool {
    max_output_bytes: usize,
}

impl SdkTool for SdkSkillTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        super::skill::Skill.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Skill])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let name = required_string(invocation.arguments(), "name")?;
            if !valid_skill_name(name) {
                return Err(SdkToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "skill name must contain only ASCII letters, digits, '-' or '_'",
                ));
            }
            if name == "rho-diagnostics" {
                authorize_request(
                    &context,
                    CapabilityRequest::skill(name, None, CapabilitySource::built_in_tool("skill")),
                )
                .await?;
                return Ok(ToolOutput::text(truncate(
                    include_str!("../builtin_skills/rho-diagnostics/SKILL.md").into(),
                    self.max_output_bytes,
                )));
            }
            let workspace = workspace(&context)?;
            let skill = crate::skills::discover(workspace.root())
                .into_iter()
                .find(|skill| skill.name == name)
                .ok_or_else(|| {
                    SdkToolError::new(
                        ToolErrorKind::InvalidArguments,
                        format!("unknown skill: {name}"),
                    )
                })?;
            let crate::skills::SkillSource::File(requested) = skill.source else {
                return Err(SdkToolError::new(
                    ToolErrorKind::Execution,
                    format!("built-in skill '{name}' is not loadable"),
                ));
            };
            let skill_directory = requested.parent().ok_or_else(|| {
                SdkToolError::new(
                    ToolErrorKind::Execution,
                    format!(
                        "skill path '{}' has no parent directory",
                        requested.display()
                    ),
                )
            })?;
            let skill_workspace = workspace
                .clone()
                .with_granted_root(skill_directory)
                .map_err(|error| SdkToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let resolved = skill_workspace
                .resolve_for_read(&requested)
                .map_err(|error| SdkToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            authorize_request(
                &context,
                CapabilityRequest::skill(
                    name,
                    Some(resolved.path().to_path_buf()),
                    CapabilitySource::built_in_tool("skill"),
                ),
            )
            .await?;
            skill_workspace.revalidate(&resolved).map_err(|error| {
                SdkToolError::new(ToolErrorKind::PolicyDenied, error.to_string())
            })?;
            let contents = tokio::fs::read_to_string(resolved.path())
                .await
                .map_err(|error| SdkToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            Ok(ToolOutput::text(truncate(contents, self.max_output_bytes)))
        })
    }
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

struct QuestionnaireTool;

impl SdkTool for QuestionnaireTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        crate::questionnaire::tool_spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let request = crate::questionnaire::parse_request(invocation.into_arguments())
                .map_err(|message| SdkToolError::new(ToolErrorKind::InvalidArguments, message))?;
            let questions = request
                .questions
                .iter()
                .map(host_question)
                .collect::<Result<Vec<_>, _>>()?;
            let title = request
                .title
                .clone()
                .unwrap_or_else(|| "questionnaire".into());
            let host_request =
                HostInputRequest::questionnaire(title, questions).map_err(map_sdk_error)?;
            let response = context
                .request_host_input(host_request)
                .await
                .map_err(map_sdk_error)?;
            let answers = response
                .answers()
                .iter()
                .map(|(id, values)| crate::questionnaire::QuestionnaireAnswer {
                    id: id.clone(),
                    answer: if values.len() == 1 {
                        serde_json::Value::String(values[0].clone())
                    } else {
                        serde_json::Value::Array(
                            values
                                .iter()
                                .cloned()
                                .map(serde_json::Value::String)
                                .collect(),
                        )
                    },
                })
                .collect();
            let content = crate::questionnaire::response_content(
                &crate::questionnaire::QuestionnaireResponse { answers },
            );
            Ok(ToolOutput::text(content).metadata(
                ToolMetadata::new().operation(OperationKind::Other("questionnaire".into())),
            ))
        })
    }
}

fn host_question(
    question: &crate::questionnaire::QuestionnaireQuestion,
) -> Result<HostQuestion, SdkToolError> {
    use crate::questionnaire::QuestionnaireQuestionKind;

    let (choices, selection) = match question.kind {
        QuestionnaireQuestionKind::Choice => (
            question
                .choices
                .iter()
                .map(|choice| HostChoice::new(choice, choice))
                .collect(),
            SelectionMode::One,
        ),
        QuestionnaireQuestionKind::MultiSelect => (
            question
                .choices
                .iter()
                .map(|choice| HostChoice::new(choice, choice))
                .collect(),
            SelectionMode::Many,
        ),
        QuestionnaireQuestionKind::Confirm => (
            vec![HostChoice::new("yes", "Yes"), HostChoice::new("no", "No")],
            SelectionMode::One,
        ),
        QuestionnaireQuestionKind::Text => {
            (vec![HostChoice::new("other", "Other")], SelectionMode::One)
        }
    };
    let mut host = HostQuestion::new(&question.id, &question.question, choices, selection)
        .map_err(map_sdk_error)?;
    if question.allow_other || matches!(question.kind, QuestionnaireQuestionKind::Text) {
        host = host.allow_other();
    }
    if let Some(help) = &question.help {
        host = host.help(help);
    }
    if let Some(default) = &question.default {
        host = host.default_value(default.clone());
    }
    if !question.required {
        host = host.optional();
    }
    Ok(host)
}

fn map_sdk_error(error: rho_sdk::Error) -> SdkToolError {
    match error {
        rho_sdk::Error::Cancelled => SdkToolError::cancelled(),
        error => SdkToolError::new(ToolErrorKind::Execution, error.to_string()),
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

    fn security(&self) -> ToolSecurity {
        security_for(&self.inner.spec().name)
    }

    fn start_metadata(&self, _arguments: &serde_json::Value) -> ToolMetadata {
        metadata_for(&self.inner.spec().name)
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            check_cancelled(&context)?;
            let spec = self.inner.spec();
            authorize_builtin(
                &spec.name,
                invocation.arguments(),
                &context,
                self.max_output_bytes,
            )
            .await?;
            let cwd = workspace_root(&context)?;
            let id = invocation.id().to_string();
            let arguments = invocation.arguments().clone();
            let app_context = AppToolContext {
                cwd: cwd.to_path_buf(),
                max_output_bytes: self.max_output_bytes,
            };
            // Bridge the tool's synchronous update callback into the SDK
            // progress channel so hosts see live output while the tool runs.
            let (update_sender, mut updates) =
                tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
            let mut on_update = move |lines: Vec<String>| {
                let _ = update_sender.send(lines);
            };
            let call = self.inner.call_with_updates_and_cancellation(
                arguments,
                app_context,
                id,
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
            let result = result.map_err(map_app_error)?;
            if !result.ok {
                return Err(SdkToolError::new(ToolErrorKind::Execution, result.content));
            }
            Ok(ToolOutput::text(result.content).metadata(metadata_for(&self.inner.spec().name)))
        })
    }
}

fn metadata_for(name: &str) -> ToolMetadata {
    let operation = match name {
        "process" => OperationKind::Execute,
        "web_search" | "fetch_content" => OperationKind::Network,
        "get_search_content" => OperationKind::Read,
        _ => OperationKind::Other(name.to_string()),
    };
    ToolMetadata::new().operation(operation)
}

fn map_app_error(error: AppToolError) -> SdkToolError {
    match &error {
        AppToolError::InvalidArguments(_) => {
            SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
        }
        AppToolError::Message(message) if message == "tool interrupted" => {
            SdkToolError::cancelled()
        }
        AppToolError::Io(_) | AppToolError::Utf8(_) | AppToolError::Message(_) => {
            SdkToolError::new(ToolErrorKind::Execution, error.to_string())
        }
    }
}

#[cfg(test)]
#[path = "sdk_registry_tests.rs"]
mod tests;
