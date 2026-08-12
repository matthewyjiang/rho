use rho_providers::model::{ContentBlock, Message};
use rho_sdk::{
    ApprovalRequest, CapabilityOperation, CapabilityRequest, CapabilitySource, NetworkTarget,
    PathScope,
};
use serde_json::Value;

/// Keep the newest tool_call records that still fit beside every user message.
///
/// Receipt: long Auto sessions accumulate hundreds of tool calls; 40 recent
/// calls is enough context for the pending decision without shipping the whole
/// investigation into every classifier turn.
const MAX_TOOL_CALL_RECORDS: usize = 40;

/// Cap serialized tool-call arguments so large write bodies do not dominate
/// classifier latency.
///
/// Receipt: path/command metadata is usually well under 200 chars; 500 leaves
/// room for short patches while cutting multi-KB file contents.
const MAX_TOOL_ARGUMENT_CHARS: usize = 500;

pub(crate) fn render_classifier_transcript(
    history: &[Message],
    pending: &ApprovalRequest,
) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    let mut tool_call_indexes = Vec::new();

    for message in history {
        match message {
            Message::System(_) => {}
            Message::User(blocks) => {
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
            Message::Assistant(blocks) => {
                append_tool_calls(&mut lines, &mut tool_call_indexes, blocks)?
            }
            Message::EnrichedAssistant(assistant) => {
                append_tool_calls(&mut lines, &mut tool_call_indexes, &assistant.content)?;
            }
            Message::AbortedAssistant(aborted) => {
                append_tool_calls(&mut lines, &mut tool_call_indexes, &aborted.content)?;
            }
            Message::ToolResult(_) => {}
        }
    }

    if tool_call_indexes.len() > MAX_TOOL_CALL_RECORDS {
        let drop_count = tool_call_indexes.len() - MAX_TOOL_CALL_RECORDS;
        let drop_set: std::collections::BTreeSet<_> =
            tool_call_indexes.into_iter().take(drop_count).collect();
        lines = lines
            .into_iter()
            .enumerate()
            .filter_map(|(index, line)| {
                if drop_set.contains(&index) {
                    None
                } else {
                    Some(line)
                }
            })
            .collect();
    }

    lines.push("pending_capability:".into());
    lines.extend(format_pending_capability(pending)?);

    Ok(lines.join("\n"))
}

fn append_tool_calls(
    lines: &mut Vec<String>,
    tool_call_indexes: &mut Vec<usize>,
    blocks: &[ContentBlock],
) -> anyhow::Result<()> {
    for block in blocks {
        let ContentBlock::ToolCall(call) = block else {
            continue;
        };
        tool_call_indexes.push(lines.len());
        lines.push(record(
            "tool_call",
            &[
                ("name", json_str(&call.name)),
                ("arguments", compact_json_value(&call.arguments)),
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
                    compact_json_value(&serde_json::to_value(invocation.arguments())?),
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

fn compact_json_value(value: &Value) -> String {
    let raw = value.to_string();
    if raw.chars().count() <= MAX_TOOL_ARGUMENT_CHARS {
        return raw;
    }
    let mut out = String::new();
    for (index, ch) in raw.chars().enumerate() {
        if index + 1 >= MAX_TOOL_ARGUMENT_CHARS {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
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
