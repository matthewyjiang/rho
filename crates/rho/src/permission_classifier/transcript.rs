use std::collections::HashMap;

use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};
use rho_sdk::model::SemanticMessage;
use rho_sdk::{
    ApprovalRequest, CapabilityOperation, CapabilityRequest, CapabilitySource, NetworkTarget,
    PathScope,
};

pub(crate) fn render_classifier_transcript(
    history: &[Message],
    pending: &ApprovalRequest,
) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    let mut pending_calls = HashMap::new();

    for message in history {
        match message.semantic() {
            // Tool images cannot supply evidence of user authorization.
            SemanticMessage::System(_) | SemanticMessage::ToolImageSupplement(_) => {}
            SemanticMessage::User(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text(text) => {
                            lines.push(record("user", &[("text", json_str(text))])?)
                        }
                        ContentBlock::Image(_) => {
                            lines.push(record("user", &[("text", json_str("[image omitted]"))])?)
                        }
                        ContentBlock::ToolCall(_) => {}
                    }
                }
            }
            SemanticMessage::Assistant(blocks) => {
                append_tool_calls(&mut lines, &mut pending_calls, blocks)?;
            }
            SemanticMessage::EnrichedAssistant(assistant) => {
                append_tool_calls(&mut lines, &mut pending_calls, &assistant.content)?;
            }
            SemanticMessage::AbortedAssistant(aborted) => {
                append_tool_calls(&mut lines, &mut pending_calls, &aborted.content)?;
            }
            SemanticMessage::ToolResult(result) => {
                if let Some(call) = pending_calls.remove(result.id.as_str()) {
                    append_questionnaire_answers(&mut lines, call, result)?;
                }
            }
        }
    }

    lines.push("pending_capability:".into());
    lines.extend(format_pending_capability(pending)?);

    Ok(lines.join("\n"))
}

fn append_tool_calls<'a>(
    lines: &mut Vec<String>,
    pending_calls: &mut HashMap<&'a str, &'a ToolCall>,
    blocks: &'a [ContentBlock],
) -> anyhow::Result<()> {
    for block in blocks {
        let ContentBlock::ToolCall(call) = block else {
            continue;
        };
        pending_calls.insert(&call.id, call);
        lines.push(record(
            "tool_call",
            &[
                ("call_id", json_str(&call.id)),
                ("name", json_str(&call.name)),
                ("arguments", call.arguments.to_string()),
            ],
        )?);
    }
    Ok(())
}

/// Only the questionnaire host-input bridge supplies answer evidence. Pair by
/// call ID, parse its structured response, and omit every other tool body.
fn append_questionnaire_answers(
    lines: &mut Vec<String>,
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
        lines.push(record(
            "questionnaire_answer",
            &[
                ("call_id", json_str(&call.id)),
                ("question_id", json_str(&question.id)),
                ("question", json_str(&question.question)),
                ("answer", answer.answer.to_string()),
            ],
        )?);
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
