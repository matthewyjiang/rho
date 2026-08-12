use rho_providers::model::{ContentBlock, Message};
use rho_sdk::{
    ApprovalRequest, CapabilityOperation, CapabilityRequest, CapabilitySource, NetworkTarget,
    PathScope,
};

pub(crate) fn render_classifier_transcript(
    history: &[Message],
    pending: &ApprovalRequest,
) -> String {
    let mut lines = Vec::new();

    for message in history {
        match message {
            Message::System(_) => {}
            Message::User(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text(text) => lines.push(format!("user: {text}")),
                        ContentBlock::Image(_) => lines.push("user: [image omitted]".into()),
                        ContentBlock::ToolCall(_) => {}
                    }
                }
            }
            Message::Assistant(blocks) => append_tool_calls(&mut lines, blocks),
            Message::EnrichedAssistant(assistant) => {
                append_tool_calls(&mut lines, &assistant.content);
            }
            Message::AbortedAssistant(aborted) => {
                append_tool_calls(&mut lines, &aborted.content);
            }
            Message::ToolResult(_) => {}
        }
    }

    lines.push("pending_capability:".into());
    lines.extend(format_pending_capability(pending));

    lines.join("\n")
}

fn append_tool_calls(lines: &mut Vec<String>, blocks: &[ContentBlock]) {
    for block in blocks {
        let ContentBlock::ToolCall(call) = block else {
            continue;
        };
        lines.push(format!(
            "tool_call: {} {}",
            call.name,
            call.arguments.to_string()
        ));
    }
}

fn format_pending_capability(pending: &ApprovalRequest) -> Vec<String> {
    let capability = pending.capability();
    let mut lines = vec![
        format!("  kind: {}", capability.kind().label()),
        format!(
            "  source: {}",
            format_capability_source(capability.source())
        ),
        format!("  reason: {}", pending.reason()),
    ];
    lines.extend(format_capability_operation(capability));
    lines
}

fn format_capability_source(source: &CapabilitySource) -> String {
    match source {
        CapabilitySource::HostProvidedTool { name } => format!("host tool {name}"),
        CapabilitySource::BuiltInTool { name } => format!("built-in tool {name}"),
        CapabilitySource::PromptConstruction => "prompt construction".into(),
        _ => "unspecified source".into(),
    }
}

fn format_capability_operation(request: &CapabilityRequest) -> Vec<String> {
    match request.operation() {
        CapabilityOperation::ReadPath { path, scope }
        | CapabilityOperation::WritePath { path, scope }
        | CapabilityOperation::DiscoverInstructions { path, scope } => {
            vec![
                format!("  path: {}", path.to_string_lossy()),
                format!("  scope: {}", format_path_scope(scope)),
            ]
        }
        CapabilityOperation::ExecuteProcess(execution) => {
            let invocation = execution.invocation();
            let mut lines = vec![format!(
                "  cwd: {}",
                execution.working_directory().to_string_lossy()
            )];
            if let Some(command) = invocation.shell_command() {
                lines.push(format!("  command: {command}"));
            } else {
                lines.push(format!(
                    "  executable: {}",
                    invocation.executable_path().to_string_lossy()
                ));
                lines.push(format!("  arguments: {:?}", invocation.arguments()));
            }
            lines
        }
        CapabilityOperation::NetworkAccess(target) => vec![format!(
            "  target: {}",
            match target {
                NetworkTarget::Url(url) => url.clone(),
                NetworkTarget::ToolManaged => "tool-managed network access".into(),
                _ => "unspecified network target".into(),
            }
        )],
        CapabilityOperation::LoadSkill { name, path } => {
            let mut lines = vec![format!("  skill: {name}")];
            if let Some(path) = path {
                lines.push(format!("  path: {}", path.to_string_lossy()));
            }
            lines
        }
        _ => Vec::new(),
    }
}

fn format_path_scope(scope: &PathScope) -> String {
    match scope {
        PathScope::PrimaryWorkspace => "primary workspace".into(),
        PathScope::GrantedRoot { root } => format!("granted root {}", root.to_string_lossy()),
        PathScope::UnrestrictedFilesystem => "unrestricted filesystem".into(),
        _ => "unspecified scope".into(),
    }
}
