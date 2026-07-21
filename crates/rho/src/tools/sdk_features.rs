//! SDK implementations for app-owned skill and host-input features.

use std::sync::Arc;

use rho_sdk::{
    tool::{
        OperationKind, Tool as SdkTool, ToolContext as SdkToolContext, ToolError as SdkToolError,
        ToolErrorKind, ToolFuture, ToolInvocation, ToolInvocationSource, ToolMetadata, ToolOutput,
        ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, HostChoice, HostInputRequest,
    HostQuestion, SelectionMode,
};
use rho_tools::{
    sdk_security::authorize_request,
    sdk_support::{required_string, workspace},
    tool::{truncate, Tool as _},
};

pub(super) fn skill_bundle(max_output_bytes: usize) -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(SdkSkillTool::new(max_output_bytes))])
}

pub(super) fn questionnaire_bundle() -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(QuestionnaireTool)])
}

impl SdkSkillTool {
    pub(super) fn new(max_output_bytes: usize) -> Self {
        Self { max_output_bytes }
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

    fn call<'a>(&'a self, invocation: ToolInvocation, context: SdkToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let invocation_source = invocation.source();
            let name = required_string(invocation.arguments(), "name")?;
            if !valid_skill_name(name) {
                return Err(SdkToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "skill name must contain only ASCII letters, digits, '-' or '_'",
                ));
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
            if skill.disable_model_invocation
                && !matches!(invocation_source, ToolInvocationSource::Host)
            {
                return Err(SdkToolError::new(
                    ToolErrorKind::PolicyDenied,
                    format!("skill '{name}' requires direct user invocation"),
                ));
            }
            let requested = match &skill.source {
                crate::skills::SkillSource::BuiltIn => {
                    authorize_request(
                        &context,
                        CapabilityRequest::skill(
                            name,
                            None,
                            CapabilitySource::built_in_tool("skill"),
                        ),
                    )
                    .await?;
                    let content = format!(
                        "Loaded skill: {name}\nSource: built in to rho\n\n{}",
                        skill.contents
                    );
                    return Ok(ToolOutput::text(truncate(content, self.max_output_bytes)));
                }
                crate::skills::SkillSource::File(requested) => requested,
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
                .resolve_for_read(requested)
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
            let content = format!(
                "Loaded skill: {name}\nSource: {}\nReferences are relative to {}.\n\n{contents}",
                crate::paths::display(requested),
                crate::paths::display(skill_directory),
            );
            Ok(ToolOutput::text(truncate(content, self.max_output_bytes)))
        })
    }
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
    if let Some(header) = &question.header {
        host = host.header(header);
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
