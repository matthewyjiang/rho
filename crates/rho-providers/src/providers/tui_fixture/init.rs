//! Deterministic /init effects, keyed on the loaded skill rather than command prose.

use rho_sdk::{
    model::{Message, ModelRequest, ModelResponse},
    ProviderError,
};

use super::{completed, completed_tool_call, last_user_text, tool_result, tool_result_for_name};

const WRITE_CALL_ID: &str = "tui-fixture-init-write";
const INSTRUCTIONS: &str =
    "# Project instructions\n\nUse fixture-init-project-rule when working in this repository.\n";

pub(super) fn intercept(
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if last_user_text(request).as_deref() == Some("fixture init instructions") {
        let loaded = request
            .messages
            .iter()
            .any(|message| matches!(message, Message::System(text) if text.contains(INSTRUCTIONS)));
        return Some(completed(if loaded {
            "fixture init instructions loaded"
        } else {
            "fixture init instructions missing"
        }));
    }
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
    Some(completed_tool_call(
        WRITE_CALL_ID,
        "write",
        serde_json::json!({"path": "AGENTS.md", "content": INSTRUCTIONS}),
    ))
}
