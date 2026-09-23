//! Fixture prompts that spawn subagents for in-place attach scenarios.

use rho_sdk::{
    model::{ContentBlock, ModelRequest, ModelResponse, ToolCall},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed, completed_tool_call, last_user_text, tool_result};

const SECOND_RAIL_AGENT_CALL_ID: &str = "tui-fixture-subagent-rail-agent-b";
const ATTACH_THEN_APPROVAL_AGENT_CALL_ID: &str = "tui-fixture-attach-then-approval-agent";
const ATTACH_THEN_APPROVAL_BASH_CALL_ID: &str = "tui-fixture-attach-then-approval-bash";
/// Must match the marker released by the `attach_view_parent_approval` scenario.
const APPROVAL_RELEASE: &str = ".rho-fixture-release-attach-approval";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    _events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    match prompt {
        "fixture two subagents"
            if tool_result(request, super::SUBAGENT_RAIL_AGENT_CALL_ID).is_none()
                && tool_result(request, SECOND_RAIL_AGENT_CALL_ID).is_none() =>
        {
            Some(Ok(ModelResponse::Assistant(vec![
                ContentBlock::ToolCall(ToolCall {
                    id: super::SUBAGENT_RAIL_AGENT_CALL_ID.into(),
                    name: "agent".into(),
                    arguments: serde_json::json!({
                        "agent_id": "worker",
                        "prompt": "fixture delay",
                    }),
                }),
                ContentBlock::ToolCall(ToolCall {
                    id: SECOND_RAIL_AGENT_CALL_ID.into(),
                    name: "agent".into(),
                    arguments: serde_json::json!({
                        "agent_id": "explorer",
                        "prompt": "fixture delay",
                    }),
                }),
            ])))
        }
        "fixture attach then approval"
            if tool_result(request, ATTACH_THEN_APPROVAL_AGENT_CALL_ID).is_none() =>
        {
            Some(Ok(ModelResponse::Assistant(vec![
                ContentBlock::Text("attach approval agent dispatched".into()),
                ContentBlock::ToolCall(ToolCall {
                    id: ATTACH_THEN_APPROVAL_AGENT_CALL_ID.into(),
                    name: "agent".into(),
                    arguments: serde_json::json!({
                        "agent_id": "worker",
                        "prompt": "fixture delay",
                    }),
                }),
            ])))
        }
        "fixture attach then approval"
            if tool_result(request, ATTACH_THEN_APPROVAL_BASH_CALL_ID).is_none() =>
        {
            // The PTY scenario releases this once the user is inside the attach
            // view, so the approval always arrives mid-attach.
            if let Err(error) =
                super::release::wait_for_release_or_cancel(APPROVAL_RELEASE, &request.cancellation)
                    .await
            {
                return Some(Err(error));
            }
            Some(completed_tool_call(
                ATTACH_THEN_APPROVAL_BASH_CALL_ID,
                "bash",
                serde_json::json!({ "command": "echo attach-approval" }),
            ))
        }
        _ => completion(request),
    }
}

fn completion(request: &ModelRequest<'_>) -> Option<Result<ModelResponse, ProviderError>> {
    let prompt = last_user_text(request)?;
    if prompt == "fixture two subagents" {
        if let (Some(worker), Some(explorer)) = (
            tool_result(request, super::SUBAGENT_RAIL_AGENT_CALL_ID),
            tool_result(request, SECOND_RAIL_AGENT_CALL_ID),
        ) {
            let worker = worker.content.lines().next().unwrap_or_default();
            let explorer = explorer.content.lines().next().unwrap_or_default();
            return Some(completed(format!(
                "two subagents dispatched: {worker}; {explorer}"
            )));
        }
    }
    if prompt == "fixture attach then approval" {
        if let Some(result) = tool_result(request, ATTACH_THEN_APPROVAL_BASH_CALL_ID) {
            return Some(completed(format!(
                "attach approval fixture finished: {}",
                result.content
            )));
        }
        if let Some(result) = tool_result(request, ATTACH_THEN_APPROVAL_AGENT_CALL_ID) {
            let receipt = result.content.lines().next().unwrap_or_default();
            return Some(completed(format!(
                "attach approval agent dispatched: {receipt}"
            )));
        }
    }
    None
}
