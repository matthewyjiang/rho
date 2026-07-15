use std::collections::BTreeMap;

use rho_sdk::{
    model::{ContextUsage, ModelUsage, ToolCall},
    tool::{OperationKind, ToolMetadata},
    HostInputRequest, HostInputResponse, RunEvent, ToolCompletion,
};

use crate::{
    questionnaire::{
        QuestionnaireAnswer, QuestionnaireQuestion, QuestionnaireQuestionKind,
        QuestionnaireRequest, QuestionnaireResponse,
    },
    tool::ToolDisplayStyle,
};

#[derive(Clone, Debug)]
pub(super) enum ViewModelEvent {
    StepStarted(usize),
    #[allow(dead_code)]
    ToolStarted {
        name: String,
        command: Option<String>,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
    },
    OutputDelta(String),
    ReasoningDelta(String),
    #[allow(dead_code)]
    ContextUsage(ContextUsage),
    Usage(ModelUsage),
    ToolUpdated {
        display_lines: Vec<String>,
    },
    ToolCallUpdated {
        display_lines: Vec<String>,
    },
    ToolFinished {
        ok: bool,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub(super) enum ViewEvent {
    Update(ViewModelEvent),
    Questionnaire(HostInputRequest),
    Notice(String),
    Completed,
    Cancelled,
    Failed(String),
    Ignored,
}

#[derive(Clone, Debug)]
struct ToolView {
    name: String,
    arguments: serde_json::Value,
    metadata: ToolMetadata,
}

#[derive(Default)]
pub(super) struct SdkEventAdapter {
    proposed: BTreeMap<String, ToolView>,
    partial_names: BTreeMap<usize, String>,
    partial_arguments: BTreeMap<usize, String>,
}

impl SdkEventAdapter {
    pub(super) fn translate(&mut self, event: RunEvent) -> ViewEvent {
        match event {
            RunEvent::Started { .. } => ViewEvent::Ignored,
            RunEvent::StepStarted { step } => ViewEvent::Update(ViewModelEvent::StepStarted(step)),
            RunEvent::AssistantTextDelta { text } => {
                ViewEvent::Update(ViewModelEvent::OutputDelta(text))
            }
            RunEvent::ReasoningDelta { text } | RunEvent::ReasoningSummaryDelta { text } => {
                ViewEvent::Update(ViewModelEvent::ReasoningDelta(text))
            }
            RunEvent::ToolCallUpdated {
                index,
                name,
                arguments_delta,
                ..
            } => {
                if let Some(name) = name {
                    self.partial_names.insert(index, name);
                }
                self.partial_arguments
                    .entry(index)
                    .or_default()
                    .push_str(&arguments_delta);
                let name = self
                    .partial_names
                    .get(&index)
                    .map(String::as_str)
                    .unwrap_or("tool");
                let arguments = self
                    .partial_arguments
                    .get(&index)
                    .map(String::as_str)
                    .unwrap_or_default();
                ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
                    display_lines: preview_lines(name, arguments),
                })
            }
            RunEvent::ToolProposed { call } => {
                let lines = proposed_lines(&call);
                self.proposed.insert(
                    call.id.clone(),
                    ToolView {
                        name: call.name,
                        arguments: call.arguments,
                        metadata: ToolMetadata::default(),
                    },
                );
                ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
                    display_lines: lines,
                })
            }
            RunEvent::ToolStarted {
                call_id,
                name,
                metadata,
            } => {
                let id = call_id.to_string();
                let tool = self.proposed.entry(id).or_insert_with(|| ToolView {
                    name: name.clone(),
                    arguments: serde_json::Value::Object(Default::default()),
                    metadata: metadata.clone(),
                });
                tool.name = name;
                tool.metadata = metadata;
                ViewEvent::Update(ViewModelEvent::ToolStarted {
                    name: tool.name.clone(),
                    command: tool.metadata.command_summary_text().map(str::to_string),
                    display_style: ToolDisplayStyle::for_tool_name(&tool.name),
                    display_lines: tool_lines(tool, None),
                })
            }
            RunEvent::ToolUpdated { call_id, progress } => {
                let id = call_id.to_string();
                let mut tool = self.proposed.get_mut(&id);
                if let Some(tool) = tool.as_deref_mut() {
                    tool.metadata = progress.presentation().clone();
                }
                let mut lines =
                    tool.map_or_else(|| vec!["tool".into()], |tool| tool_lines(tool, None));
                if !progress.text().trim().is_empty() {
                    lines.push(progress.text().to_string());
                }
                if let (Some(completed), Some(total)) =
                    (progress.completed_units(), progress.total_units())
                {
                    lines.push(format!("progress: {completed}/{total}"));
                }
                ViewEvent::Update(ViewModelEvent::ToolUpdated {
                    display_lines: lines,
                })
            }
            RunEvent::ToolFinished { call_id, result } => {
                let id = call_id.to_string();
                let tool = self.proposed.remove(&id).unwrap_or_else(|| ToolView {
                    name: "tool".into(),
                    arguments: serde_json::Value::Object(Default::default()),
                    metadata: ToolMetadata::default(),
                });
                let (ok, content, metadata) = match result {
                    ToolCompletion::Success(output) => (
                        true,
                        output.content().to_string(),
                        output.presentation().clone(),
                    ),
                    ToolCompletion::Failure(error) => {
                        (false, error.message().to_string(), ToolMetadata::default())
                    }
                    ToolCompletion::Unavailable => {
                        (false, "tool is unavailable".into(), ToolMetadata::default())
                    }
                    _ => (false, "unknown tool result".into(), ToolMetadata::default()),
                };
                let mut completed = tool;
                if metadata != ToolMetadata::default() {
                    completed.metadata = metadata;
                }
                ViewEvent::Update(ViewModelEvent::ToolFinished {
                    ok,
                    display_style: ToolDisplayStyle::for_tool_name(&completed.name),
                    display_lines: tool_lines(&completed, Some(&content)),
                })
            }
            RunEvent::UsageUpdated { usage } => ViewEvent::Update(ViewModelEvent::Usage(usage)),
            RunEvent::ProviderActivity { kind, detail } => {
                if kind == "web_search" {
                    ViewEvent::Update(ViewModelEvent::ToolFinished {
                        ok: true,
                        display_style: ToolDisplayStyle::web(),
                        display_lines: vec!["web search".into(), detail],
                    })
                } else {
                    ViewEvent::Notice(format!("{kind}: {detail}"))
                }
            }
            RunEvent::ProviderContextUpdated { .. } => ViewEvent::Ignored,
            RunEvent::HostInputRequested { request } => ViewEvent::Questionnaire(request),
            RunEvent::CompactionStarted { .. } => {
                ViewEvent::Notice("compacting conversation context".into())
            }
            RunEvent::CompactionCompleted { outcome, .. } => ViewEvent::Notice(format!(
                "compacted conversation context ({} to {} messages)",
                outcome.previous_messages(),
                outcome.current_messages()
            )),
            RunEvent::Completed { .. } => ViewEvent::Completed,
            RunEvent::Cancelled { .. } => ViewEvent::Cancelled,
            RunEvent::Failed { message, .. } => ViewEvent::Failed(message),
            _ => ViewEvent::Ignored,
        }
    }
}

pub(super) fn questionnaire_request(request: &HostInputRequest) -> QuestionnaireRequest {
    QuestionnaireRequest {
        title: (!request.title().is_empty()).then(|| request.title().to_string()),
        reason: None,
        questions: request
            .questions()
            .iter()
            .map(|question| QuestionnaireQuestion {
                id: question.id().to_string(),
                question: question.prompt().to_string(),
                help: question.help_text().map(str::to_string),
                default: question.default_value_ref().cloned(),
                kind: match question.selection() {
                    rho_sdk::SelectionMode::One => QuestionnaireQuestionKind::Choice,
                    rho_sdk::SelectionMode::Many => QuestionnaireQuestionKind::MultiSelect,
                    _ => QuestionnaireQuestionKind::Choice,
                },
                required: question.is_required(),
                choices: question
                    .choices()
                    .iter()
                    .map(|choice| choice.label().to_string())
                    .collect(),
                allow_other: question.permits_other(),
            })
            .collect(),
    }
}

pub(super) fn host_response(response: QuestionnaireResponse) -> HostInputResponse {
    response.answers.into_iter().fold(
        HostInputResponse::new(),
        |response, QuestionnaireAnswer { id, answer }| {
            let values = match answer {
                serde_json::Value::Array(values) => {
                    values.into_iter().map(answer_text).collect::<Vec<_>>()
                }
                value => vec![answer_text(value)],
            };
            response.answer(id, values)
        },
    )
}

fn answer_text(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value,
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn preview_lines(name: &str, arguments: &str) -> Vec<String> {
    let mut lines = vec![name.to_string()];
    if !arguments.trim().is_empty() {
        lines.push(arguments.to_string());
    }
    lines
}

fn proposed_lines(call: &ToolCall) -> Vec<String> {
    let mut lines = vec![call.name.clone()];
    if call.arguments != serde_json::Value::Object(Default::default()) {
        lines.push(call.arguments.to_string());
    }
    lines
}

fn tool_lines(tool: &ToolView, content: Option<&str>) -> Vec<String> {
    let mut lines = vec![tool.name.clone()];
    if let Some(command) = tool.metadata.command_summary_text() {
        lines.push(command.to_string());
    }
    for path in tool.metadata.affected_paths() {
        lines.push(path.display().to_string());
    }
    for url in tool.metadata.urls() {
        lines.push(url.clone());
    }
    if let Some(diff) = tool.metadata.unified_diff() {
        lines.push(diff.to_string());
    }
    if lines.len() == 1 && tool.arguments != serde_json::Value::Object(Default::default()) {
        lines.push(tool.arguments.to_string());
    }
    if let Some(content) = content.filter(|content| !content.trim().is_empty()) {
        lines.push(content.to_string());
    }
    if matches!(tool.metadata.operation_kind(), Some(OperationKind::Network)) && lines.len() == 1 {
        lines.push("network request".into());
    }
    lines
}

#[cfg(test)]
#[path = "event_adapter_tests.rs"]
mod tests;
