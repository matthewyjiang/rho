use std::collections::HashMap;

use crate::history_message::HistoryMessage;
use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};
use rho_sdk::{
    ApprovalRequest, CapabilityOperation, CapabilityRequest, CapabilitySource, NetworkTarget,
    PathScope,
};

use super::budget::{
    cap_string_leaves, fit_transcript, Retention, TranscriptBudget, TranscriptLine,
    COMPLETED_CALL_STRING_CAP_CHARS,
};

/// One transcript record in history order. Tool-call lines are rendered after
/// the walk, once each occurrence's lifecycle is known.
enum Entry {
    Line(TranscriptLine),
    Call(usize),
}

/// Lifecycle of one tool-call occurrence.
///
/// Call IDs are not unique across responses (lenient adapters reuse
/// `call_{index}`). A result answers only the latest executable occurrence of
/// its ID, and only once, so duplicate results cannot reach older calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallLifecycle {
    Unanswered,
    Answered,
    /// Part of an aborted turn; it never ran and never will.
    Aborted,
}

struct CallOccurrence<'a> {
    call: &'a ToolCall,
    lifecycle: CallLifecycle,
}

/// [`render_with_pending_call`] with the ID the SDK attached to `pending`.
#[cfg(test)]
pub(crate) fn render_classifier_transcript(
    history: &[Message],
    pending: &ApprovalRequest,
    budget: TranscriptBudget,
) -> anyhow::Result<String> {
    let pending_call_id = pending.tool_call_id().map(|id| id.as_str());
    render_with_pending_call(history, pending, pending_call_id, budget)
}

/// Renders the classifier transcript and fits it into `budget`.
///
/// Answered and aborted calls are capped and droppable. Unanswered calls and
/// answered calls sharing `pending_call_id` stay whole, because their
/// arguments may describe the action being classified. Over-budget failures
/// surface as a [`super::TranscriptOverBudget`] inside the returned error.
///
/// The ID is a parameter because only the SDK can attach one to an
/// [`ApprovalRequest`]; the classifier eval supplies replayed call IDs.
pub(super) fn render_with_pending_call(
    history: &[Message],
    pending: &ApprovalRequest,
    pending_call_id: Option<&str>,
    budget: TranscriptBudget,
) -> anyhow::Result<String> {
    let mut tail = vec!["pending_capability:".to_owned()];
    tail.extend(format_pending_capability(pending)?);
    render(history, pending_call_id.as_slice(), tail, budget)
}

/// One request of a batch the agent made at once, under the ID its question
/// asks about, which is not a tool-call ID.
pub(super) struct LabeledPending<'a> {
    pub label: &'a str,
    pub pending: &'a ApprovalRequest,
    pub call_id: Option<&'a str>,
}

/// [`render_with_pending_call`] for several pending requests. Each section is
/// headed by its label and names the tool call that made it, when known, so
/// requests with identical capabilities stay bound to their own calls.
pub(super) fn render_with_pending_calls(
    history: &[Message],
    pendings: &[LabeledPending<'_>],
    budget: TranscriptBudget,
) -> anyhow::Result<String> {
    let mut tail = Vec::new();
    for pending in pendings {
        tail.push(format!("pending_capability {}:", pending.label));
        if let Some(call_id) = pending.call_id {
            tail.push(field("call_id", json_str(call_id)));
        }
        tail.extend(format_pending_capability(pending.pending)?);
    }
    let call_ids: Vec<&str> = pendings
        .iter()
        .filter_map(|pending| pending.call_id)
        .collect();
    render(history, &call_ids, tail, budget)
}

/// The history lines followed by `tail`, fitted into `budget`. Answered calls
/// whose ID is in `pending_call_ids` stay whole.
fn render(
    history: &[Message],
    pending_call_ids: &[&str],
    tail: Vec<String>,
    budget: TranscriptBudget,
) -> anyhow::Result<String> {
    let mut entries = Vec::new();
    let mut calls = Vec::new();
    let mut latest_executable: HashMap<&str, usize> = HashMap::new();

    for message in history {
        match HistoryMessage::of(message) {
            // Tool images and model-written compaction summaries cannot
            // supply evidence of user authorization.
            HistoryMessage::System(_)
            | HistoryMessage::ToolImageSupplement(_)
            | HistoryMessage::CompactionSummary(_) => {}
            HistoryMessage::User(blocks) => {
                for block in blocks {
                    let text = match block {
                        ContentBlock::Text(text) => text.as_str(),
                        ContentBlock::Image(_) => "[image omitted]",
                        ContentBlock::ToolCall(_) => continue,
                    };
                    entries.push(Entry::Line(required(record(
                        "user",
                        &[("text", json_str(text))],
                    )?)));
                }
            }
            HistoryMessage::Assistant(blocks) => {
                let lifecycle = CallLifecycle::Unanswered;
                push_calls(
                    &mut entries,
                    &mut calls,
                    &mut latest_executable,
                    blocks,
                    lifecycle,
                );
            }
            HistoryMessage::EnrichedAssistant(assistant) => {
                let lifecycle = CallLifecycle::Unanswered;
                let blocks = &assistant.content;
                push_calls(
                    &mut entries,
                    &mut calls,
                    &mut latest_executable,
                    blocks,
                    lifecycle,
                );
            }
            HistoryMessage::AbortedAssistant(aborted) => {
                let lifecycle = CallLifecycle::Aborted;
                let blocks = &aborted.content;
                push_calls(
                    &mut entries,
                    &mut calls,
                    &mut latest_executable,
                    blocks,
                    lifecycle,
                );
            }
            HistoryMessage::ToolResult(result) => {
                let Some(&index) = latest_executable.get(result.id.as_str()) else {
                    continue;
                };
                let occurrence: &mut CallOccurrence = &mut calls[index];
                if occurrence.lifecycle != CallLifecycle::Unanswered {
                    continue;
                }
                occurrence.lifecycle = CallLifecycle::Answered;
                append_questionnaire_answers(&mut entries, occurrence.call, result)?;
            }
        }
    }

    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        lines.push(match entry {
            Entry::Line(line) => line,
            Entry::Call(index) => {
                let occurrence = &calls[index];
                let in_flight = match occurrence.lifecycle {
                    CallLifecycle::Unanswered => true,
                    // A detached job can ask after its call was answered, and
                    // reused IDs make the asking occurrence ambiguous, so
                    // every answered match of the pending ID stays whole.
                    CallLifecycle::Answered => {
                        pending_call_ids.contains(&occurrence.call.id.as_str())
                    }
                    CallLifecycle::Aborted => false,
                };
                let retention = if in_flight {
                    Retention::Required
                } else {
                    Retention::Droppable
                };
                tool_call_line(occurrence.call, retention)?
            }
        });
    }

    Ok(fit_transcript(lines, tail, budget)?)
}

/// Aborted calls never ran, so they never become the target of a result.
fn push_calls<'a>(
    entries: &mut Vec<Entry>,
    calls: &mut Vec<CallOccurrence<'a>>,
    latest_executable: &mut HashMap<&'a str, usize>,
    blocks: &'a [ContentBlock],
    lifecycle: CallLifecycle,
) {
    for block in blocks {
        let ContentBlock::ToolCall(call) = block else {
            continue;
        };
        let index = calls.len();
        calls.push(CallOccurrence { call, lifecycle });
        if lifecycle != CallLifecycle::Aborted {
            latest_executable.insert(&call.id, index);
        }
        entries.push(Entry::Call(index));
    }
}

/// Droppable calls are history, not the action under review, so their
/// string arguments are capped.
fn tool_call_line(call: &ToolCall, retention: Retention) -> anyhow::Result<TranscriptLine> {
    let arguments = match retention {
        Retention::Required => call.arguments.clone(),
        Retention::Droppable => cap_string_leaves(&call.arguments, COMPLETED_CALL_STRING_CAP_CHARS),
    };
    Ok(TranscriptLine {
        text: record(
            "tool_call",
            &[
                ("call_id", json_str(&call.id)),
                ("name", json_str(&call.name)),
                ("arguments", arguments.to_string()),
            ],
        )?,
        retention,
    })
}

fn required(text: String) -> TranscriptLine {
    TranscriptLine {
        text,
        retention: Retention::Required,
    }
}

/// Only the questionnaire host-input bridge supplies answer evidence. Pair by
/// call occurrence, parse its structured response, and omit every other tool body.
fn append_questionnaire_answers(
    entries: &mut Vec<Entry>,
    call: &ToolCall,
    result: &ToolResult,
) -> anyhow::Result<()> {
    if call.name != crate::questionnaire::TOOL_NAME || !result.ok {
        return Ok(());
    }
    let Ok(request) = crate::questionnaire::parse_request(call.arguments.clone()) else {
        return Ok(());
    };
    let Ok(response) =
        serde_json::from_str::<crate::questionnaire::QuestionnaireResponse>(&result.content)
    else {
        return Ok(());
    };
    for answer in response.answers {
        let Some(question) = request.questions.iter().find(|q| q.id == answer.id) else {
            continue;
        };
        entries.push(Entry::Line(required(record(
            "questionnaire_answer",
            &[
                ("call_id", json_str(&call.id)),
                ("question_id", json_str(&question.id)),
                ("question", json_str(&question.question)),
                ("answer", answer.answer.to_string()),
            ],
        )?)));
    }
    Ok(())
}

fn format_pending_capability(pending: &ApprovalRequest) -> anyhow::Result<Vec<String>> {
    let capability = pending.capability();
    let mut lines = vec![
        field("kind", json_str(capability.kind().label())),
        field(
            "source",
            json_str(&format_capability_source(capability.source())?),
        ),
        field("reason", json_str(pending.reason())),
    ];
    lines.extend(format_capability_operation(capability)?);
    Ok(lines)
}

fn format_capability_source(source: &CapabilitySource) -> anyhow::Result<String> {
    match source {
        CapabilitySource::HostProvidedTool { name } => Ok(format!("host tool {name}")),
        CapabilitySource::BuiltInTool { name } => Ok(format!("built-in tool {name}")),
        CapabilitySource::PromptConstruction => Ok("prompt construction".into()),
        _ => anyhow::bail!("unsupported capability source for classifier transcript"),
    }
}

fn format_capability_operation(request: &CapabilityRequest) -> anyhow::Result<Vec<String>> {
    match request.operation() {
        CapabilityOperation::ReadPath { path, scope }
        | CapabilityOperation::WritePath { path, scope }
        | CapabilityOperation::DiscoverInstructions { path, scope } => Ok(vec![
            field("path", json_str(&path.to_string_lossy())),
            field("scope", json_str(&format_path_scope(scope)?)),
        ]),
        CapabilityOperation::ExecuteProcess(execution) => {
            let invocation = execution.invocation();
            let mut lines = vec![field(
                "cwd",
                json_str(&execution.working_directory().to_string_lossy()),
            )];
            if let Some(command) = invocation.shell_command() {
                lines.push(field("command", json_str(command)));
            } else {
                lines.push(field(
                    "executable",
                    json_str(&invocation.executable_path().to_string_lossy()),
                ));
                lines.push(field(
                    "arguments",
                    serde_json::to_string(invocation.arguments())?,
                ));
            }
            Ok(lines)
        }
        CapabilityOperation::NetworkAccess(target) => {
            let target = match target {
                NetworkTarget::Url(url) => url.clone(),
                NetworkTarget::ToolManaged => "tool-managed network access".into(),
                _ => anyhow::bail!("unsupported network target for classifier transcript"),
            };
            Ok(vec![field("target", json_str(&target))])
        }
        CapabilityOperation::LoadSkill { name, path } => {
            let mut lines = vec![field("skill", json_str(name))];
            if let Some(path) = path {
                lines.push(field("path", json_str(&path.to_string_lossy())));
            }
            Ok(lines)
        }
        _ => anyhow::bail!("unsupported capability operation for classifier transcript"),
    }
}

fn format_path_scope(scope: &PathScope) -> anyhow::Result<String> {
    match scope {
        PathScope::PrimaryWorkspace => Ok("primary workspace".into()),
        PathScope::GrantedRoot { root } => Ok(format!("granted root {}", root.to_string_lossy())),
        PathScope::UnrestrictedFilesystem => Ok("unrestricted filesystem".into()),
        _ => anyhow::bail!("unsupported path scope for classifier transcript"),
    }
}

fn record(kind: &str, fields: &[(&str, String)]) -> anyhow::Result<String> {
    let mut parts = vec![json_str(kind)];
    for (name, value) in fields {
        parts.push(format!("{name}={value}"));
    }
    Ok(parts.join(" "))
}

fn field(name: &str, value: String) -> String {
    format!("  {name}: {value}")
}

fn json_str(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}
