//! SDK implementations for app-owned skill and host-input features.

use std::{path::Path, sync::Arc};

use rho_sdk::{
    tool::{
        OperationKind, PreparedToolInvocation, Tool as SdkTool, ToolContext as SdkToolContext,
        ToolError as SdkToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolInvocationSource,
        ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolResource,
        ToolResourceAccess, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, HostChoice, HostInputRequest,
    HostQuestion, SelectionMode,
};
use rho_tools::{
    sdk_support::required_string,
    tool::{truncate, Tool as _},
};

pub(super) fn skill_bundle(max_output_bytes: usize) -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(SdkSkillTool::new(max_output_bytes))])
}

pub(super) fn questionnaire_bundle() -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(QuestionnaireTool)])
}

pub(crate) fn message_parent_bundle(
    poster: Arc<dyn crate::app::subagent_notice::NoticePoster>,
) -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(MessageParentTool { poster })])
}

impl SdkSkillTool {
    pub(super) fn new(max_output_bytes: usize) -> Self {
        Self { max_output_bytes }
    }

    /// Shared preparation for filesystem-backed skills: loose `File` skills
    /// and plugin skills. The workspace grants only the skill directory, so
    /// resource access stays inside the skill's permitted root.
    fn prepare_fs_skill(
        &self,
        name: &str,
        source_display: String,
        requested: &Path,
        skill_directory: &Path,
        context: &ToolPreparationContext,
    ) -> Result<PreparedToolInvocation<'_>, SdkToolError> {
        let workspace = preparation_workspace(context)?;
        let skill_workspace = workspace
            .clone()
            .with_granted_root(skill_directory)
            .map_err(|error| SdkToolError::new(ToolErrorKind::Execution, error.to_string()))?;
        let resolved = skill_workspace
            .resolve_for_read(requested)
            .map_err(|error| SdkToolError::new(ToolErrorKind::Execution, error.to_string()))?;
        let capability = CapabilityRequest::skill(
            name,
            Some(resolved.path().to_path_buf()),
            CapabilitySource::built_in_tool("skill"),
        );
        let access = ToolResourceAccess::shared(ToolResource::workspace_path(resolved.path()));
        let directory_display = crate::paths::display(skill_directory);
        let max_output_bytes = self.max_output_bytes;
        let name = name.to_string();
        let metadata = ToolMetadata::new().operation(OperationKind::Read);
        Ok(PreparedToolInvocation::resource_aware(
            [access],
            [capability],
            metadata,
            move |_context| {
                Box::pin(async move {
                    skill_workspace.revalidate(&resolved).map_err(|error| {
                        SdkToolError::new(ToolErrorKind::PolicyDenied, error.to_string())
                    })?;
                    let contents =
                        tokio::fs::read_to_string(resolved.path())
                            .await
                            .map_err(|error| {
                                SdkToolError::new(ToolErrorKind::Execution, error.to_string())
                            })?;
                    let content = format!(
                        "Loaded skill: {name}\nSource: {source_display}\nReferences are relative to {directory_display}.\n\n{contents}"
                    );
                    Ok(ToolOutput::text(truncate(content, max_output_bytes)))
                })
            },
        ))
    }
}

pub(super) struct SdkSkillTool {
    max_output_bytes: usize,
}

impl SdkTool for SdkSkillTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        super::skill::Skill.spec()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Skill])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        Box::pin(async move {
            let invocation_source = invocation.source();
            let name = required_string(invocation.arguments(), "name")?.to_string();
            if !valid_skill_name(&name) {
                return Err(SdkToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "skill name must contain only ASCII letters, digits, '-' or '_'",
                ));
            }
            let skill = match crate::skills::find_builtin(&name) {
                Some(skill) => skill,
                None => {
                    let workspace = preparation_workspace(&context)?;
                    crate::skills::discover(workspace.root())
                        .into_iter()
                        .find(|skill| skill.name == name)
                        .ok_or_else(|| {
                            SdkToolError::new(
                                ToolErrorKind::InvalidArguments,
                                format!("unknown skill: {name}"),
                            )
                        })?
                }
            };
            if skill.disable_model_invocation
                && !matches!(invocation_source, ToolInvocationSource::Host)
            {
                return Err(SdkToolError::new(
                    ToolErrorKind::PolicyDenied,
                    format!("skill '{name}' requires direct user invocation"),
                ));
            }
            let source_display = skill.source.to_string();
            match skill.source {
                crate::skills::SkillSource::BuiltIn => {
                    let metadata = ToolMetadata::new().operation(OperationKind::Read);
                    let capability = CapabilityRequest::skill(
                        &name,
                        None,
                        CapabilitySource::built_in_tool("skill"),
                    );
                    let access = ToolResourceAccess::shared(ToolResource::opaque(
                        "rho.skill.builtin",
                        &name,
                    ));
                    let content = truncate(
                        format!(
                            "Loaded skill: {name}\nSource: {source_display}\n\n{}",
                            skill.contents
                        ),
                        self.max_output_bytes,
                    );
                    Ok(PreparedToolInvocation::resource_aware(
                        [access],
                        [capability],
                        metadata,
                        move |_context| Box::pin(async move { Ok(ToolOutput::text(content)) }),
                    ))
                }
                crate::skills::SkillSource::Filesystem { skill_file, .. } => {
                    let skill_directory = skill_file.parent().ok_or_else(|| {
                        SdkToolError::new(
                            ToolErrorKind::Execution,
                            format!(
                                "skill path '{}' has no parent directory",
                                skill_file.display()
                            ),
                        )
                    })?;
                    self.prepare_fs_skill(
                        &name,
                        source_display,
                        &skill_file,
                        skill_directory,
                        &context,
                    )
                }
            }
        })
    }
}

fn preparation_workspace(
    context: &ToolPreparationContext,
) -> Result<&rho_sdk::Workspace, SdkToolError> {
    context.workspace().ok_or_else(|| {
        SdkToolError::new(
            ToolErrorKind::Execution,
            "skill requires a configured workspace",
        )
    })
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) struct QuestionnaireTool;

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

/// Non-blocking plain-text notice from a delegated child to its parent.
struct MessageParentTool {
    poster: Arc<dyn crate::app::subagent_notice::NoticePoster>,
}

impl SdkTool for MessageParentTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: "message_parent".into(),
            description: "Send a short plain-text notice to the parent session without waiting for a reply. Use for blockers, findings, or status the parent should see at its next turn boundary. Do not use this for questions that need an answer - use questionnaire for those. Keep the message under 8 KiB.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Plain-text notice for the parent session"
                    }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let arguments = invocation.into_arguments();
            let message = required_string(&arguments, "message").map_err(|error| {
                SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
            })?;
            let message = crate::app::subagent_notice::validate_message_text(
                message,
                crate::app::subagent_notice::MAX_NOTICE_BYTES,
            )
            .map_err(|error| {
                SdkToolError::new(ToolErrorKind::InvalidArguments, error.to_string())
            })?;
            self.poster
                .post(message)
                .map_err(|error| SdkToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            Ok(
                ToolOutput::text("notice queued for the parent session").metadata(
                    ToolMetadata::new().operation(OperationKind::Other("message_parent".into())),
                ),
            )
        })
    }
}

fn host_question(
    question: &crate::questionnaire::QuestionnaireQuestion,
) -> Result<HostQuestion, SdkToolError> {
    use crate::questionnaire::QuestionnaireQuestionKind;

    let selection = match question.kind {
        QuestionnaireQuestionKind::MultiSelect => SelectionMode::Many,
        QuestionnaireQuestionKind::Choice
        | QuestionnaireQuestionKind::Confirm
        | QuestionnaireQuestionKind::Text => SelectionMode::One,
    };
    let choices = match question.kind {
        QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect => question
            .choices
            .iter()
            .map(|choice| {
                let host = HostChoice::new(&choice.label, &choice.label);
                match &choice.description {
                    Some(description) => host.description(description),
                    None => host,
                }
            })
            .collect(),
        QuestionnaireQuestionKind::Confirm => {
            vec![HostChoice::new("yes", "Yes"), HostChoice::new("no", "No")]
        }
        QuestionnaireQuestionKind::Text => vec![HostChoice::new("other", "Other")],
    };
    let mut host = HostQuestion::new(&question.id, &question.question, choices, selection)
        .map_err(map_sdk_error)?;
    if question.allow_other || matches!(question.kind, QuestionnaireQuestionKind::Text) {
        host = host.allow_other();
    }
    if let Some(header) = &question.header {
        host = host.header(header);
    }
    if let Some(help) = &question.help {
        host = host.help(help);
    }
    if let Some(default) = &question.default {
        host = host.default_value(default.clone());
    }
    host = host.default_selection(question.default_selection.into());
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
