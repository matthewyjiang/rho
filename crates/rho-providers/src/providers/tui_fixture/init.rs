//! Deterministic /init effects, keyed on the loaded skill rather than command prose.

use rho_sdk::{
    model::{Message, ModelRequest, ModelResponse},
    ProviderError,
};

use super::{completed, completed_tool_call, last_user_text, tool_result, tool_result_for_name};

const READ_CALL_ID: &str = "tui-fixture-init-read";
const WRITE_CALL_ID: &str = "tui-fixture-init-write";
const INSTRUCTIONS: &str =
    "# Project instructions\n\nUse fixture-init-project-rule when working in this repository.\n";

pub(super) fn intercept(
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    let user = last_user_text(request)?;
    if let Some(phase) = user.strip_prefix("fixture init instructions") {
        let loaded = request
            .messages
            .iter()
            .any(|message| matches!(message, Message::System(text) if text.contains(INSTRUCTIONS)));
        let state = if loaded { "loaded" } else { "missing" };
        return Some(completed(format!(
            "fixture init instructions {state}{phase}"
        )));
    }
    // Onboarding turns carry an explicit target rather than relying on cwd.
    let target_data = user
        .lines()
        .find_map(|line| line.strip_prefix("Target path: "))?;
    let skill = tool_result_for_name(request, "skill")?;
    if !skill.ok || skill.content.lines().next() != Some("Loaded skill: rho-init") {
        return None;
    }
    if let Some(result) = tool_result(request, WRITE_CALL_ID) {
        return Some(completed(if result.ok {
            "fixture init wrote AGENTS.md".to_string()
        } else {
            format!("fixture init write failed: {}", result.content)
        }));
    }
    // /init encodes path data as a JSON string, including quotes and controls.
    let Ok(target) = serde_json::from_str::<String>(target_data) else {
        return Some(completed("fixture init missing valid target path"));
    };
    let exists = match std::path::Path::new(&target).try_exists() {
        Ok(exists) => exists,
        Err(error) => return Some(completed(format!("fixture init target failed: {error}"))),
    };
    if !exists {
        return Some(completed_tool_call(
            WRITE_CALL_ID,
            "write",
            serde_json::json!({"path": target, "content": INSTRUCTIONS}),
        ));
    }
    let Some(read) = tool_result(request, READ_CALL_ID) else {
        return Some(completed_tool_call(
            READ_CALL_ID,
            "read_file",
            serde_json::json!({"path": target}),
        ));
    };
    if !read.ok {
        return Some(completed(format!(
            "fixture init read failed: {}",
            read.content
        )));
    }
    // Use the active editor, never write over existing guidance. The hashline
    // editor gets its live snapshot header from the actual read tool result.
    if request.tools.iter().any(|tool| tool.name == "edit") {
        let Some(header) = read
            .content
            .lines()
            .find(|line| line.starts_with('[') && line.ends_with(']') && line.contains('#'))
        else {
            return Some(completed("fixture init read missing snapshot header"));
        };
        let body = INSTRUCTIONS
            .lines()
            .map(|line| format!("+{line}\n"))
            .collect::<String>();
        return Some(completed_tool_call(
            WRITE_CALL_ID,
            "edit",
            serde_json::json!({"input": format!("{header}\nPUT >$:\n+\n{body}")}),
        ));
    }
    let contents = match std::fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) => return Some(completed(format!("fixture init read failed: {error}"))),
    };
    let Some(anchor) = contents.lines().last().filter(|line| !line.is_empty()) else {
        return Some(completed(
            "fixture init existing target missing edit anchor",
        ));
    };
    if request.tools.iter().any(|tool| tool.name == "str_replace") {
        return Some(completed_tool_call(
            WRITE_CALL_ID,
            "str_replace",
            serde_json::json!({
                "path": target,
                "old_string": anchor,
                "new_string": format!("{anchor}\n\n{}", INSTRUCTIONS.trim_end()),
            }),
        ));
    }
    if request.tools.iter().any(|tool| tool.name == "apply_patch") {
        let body = INSTRUCTIONS
            .lines()
            .map(|line| format!("+{line}\n"))
            .collect::<String>();
        return Some(completed_tool_call(
            WRITE_CALL_ID,
            "apply_patch",
            serde_json::json!({"input": format!("*** Begin Patch\n*** Update File: {target}\n@@\n {anchor}\n+\n{body}*** End Patch")}),
        ));
    }
    Some(completed("fixture init missing live file-edit tool"))
}
