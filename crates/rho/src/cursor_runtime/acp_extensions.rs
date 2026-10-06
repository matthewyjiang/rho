//! Answers to Cursor's blocking ACP extensions and wire-diff normalization.
//! Nonblocking todos/task/image requests tolerate method_not_found in Cursor.

use agent_client_protocol::schema::v1::{SessionUpdate, ToolCallContent};
use serde::Serialize;
use serde_json::Value;

use crate::permission::PermissionMode;

#[derive(Serialize)]
struct ExtensionReply {
    outcome: ExtensionOutcome,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ExtensionOutcome {
    Accepted,
    Skipped { reason: &'static str },
    Rejected { reason: &'static str },
}

/// Payload-independent headless policy. No params are read: even an empty or
/// unfamiliar question/plan payload gets an answer rather than blocking a run.
/// Only known blocking methods are claimed; None delegates method_not_found.
pub(crate) fn answer(mode: PermissionMode, method: &str, _params: &Value) -> Option<Value> {
    let outcome = match method {
        "cursor/ask_question" => ExtensionOutcome::Skipped {
            reason: "no interactive user; pick the best option and continue",
        },
        "cursor/create_plan" => match mode {
            PermissionMode::Bypass => ExtensionOutcome::Accepted,
            PermissionMode::Plan
            | PermissionMode::Auto
            | PermissionMode::AllowEdits
            | PermissionMode::Supervised => ExtensionOutcome::Rejected {
                reason: "plan-only run: report the plan as your final answer",
            },
        },
        _ => return None,
    };
    Some(
        serde_json::to_value(ExtensionReply { outcome })
            .expect("static Cursor extension reply serializes"),
    )
}

/// Cursor encodes a new file with a pseudo-diff header. Normalize only that
/// sentinel, preserving existing-file diffs, content order, and all metadata.
pub(crate) fn normalize_update(mut update: SessionUpdate) -> SessionUpdate {
    let content = match &mut update {
        SessionUpdate::ToolCall(call) => Some(&mut call.content),
        SessionUpdate::ToolCallUpdate(call) => call.fields.content.as_mut(),
        _ => None,
    };
    if let Some(content) = content {
        for item in content {
            match item {
                ToolCallContent::Diff(diff) if diff.old_text.as_deref() == Some("-- /dev/null") => {
                    diff.old_text = None;
                    if diff.new_text.starts_with("++ b/") {
                        diff.new_text = diff
                            .new_text
                            .split_once('\n')
                            .map_or("", |(_, text)| text)
                            .to_owned();
                    }
                }
                _ => {}
            }
        }
    }
    update
}

#[cfg(test)]
#[path = "acp_extensions_tests.rs"]
mod tests;
