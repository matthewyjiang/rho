use rho_providers::model::{ContentBlock, Message};
use rho_sdk::{
    ApprovalRequest, CapabilityOperation, CapabilityRequest, CapabilitySource, NetworkTarget,
    PathScope,
};

pub(crate) fn render_classifier_transcript(
    history: &[Message],
    pending: &ApprovalRequest,
) -> anyhow::Result<String> {
    let mut lines = Vec::new();

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
            Message::Assistant(blocks) => append_tool_calls(&mut lines, blocks)?,
            Message::EnrichedAssistant(assistant) => {
                append_tool_calls(&mut lines, &assistant.content)?;
            }
            Message::AbortedAssistant(aborted) => {
                append_tool_calls(&mut lines, &aborted.content)?;
            }
            Message::ToolResult(_) => {}
        }
    }

    lines.push("pending_capability:".into());
    lines.extend(format_pending_capability(pending)?);

    Ok(lines.join("\n"))
}

fn append_tool_calls(lines: &mut Vec<String>, blocks: &[ContentBlock]) -> anyhow::Result<()> {
    for block in blocks {
        let ContentBlock::ToolCall(call) = block else {
            continue;
        };
        lines.push(record(
            "tool_call",
            &[
                ("name", json_str(&call.name)),
                ("arguments", call.arguments.to_string()),
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
