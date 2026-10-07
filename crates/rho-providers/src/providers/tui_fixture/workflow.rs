//! Real workflow plan/run tools; the PTY seeds and releases the command DAG.

use rho_sdk::{
    model::{ModelRequest, ModelResponse},
    ProviderError,
};
use serde_json::json;

use super::{completed, completed_tool_call, tool_result};

const PLAN_CALL: &str = "tui-fixture-workflow-plan";
const RUN_CALL: &str = "tui-fixture-workflow-run";

pub(super) fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt.starts_with("[workflow notification]") && prompt.contains("(pty-background)") {
        let run_id = prompt.lines().find_map(|line| {
            line.strip_prefix("workflow ")?
                .split_once(" (pty-background)")
                .map(|(id, _)| id)
        })?;
        return Some(completed(format!(
            "workflow fixture completion incorporated {run_id}"
        )));
    }
    if prompt != "fixture workflow background" {
        return None;
    }
    if let Some(result) = tool_result(request, RUN_CALL) {
        return Some(if result.ok {
            completed("workflow fixture dispatched")
        } else {
            completed(format!("workflow fixture run failed: {}", result.content))
        });
    }
    let Some(result) = tool_result(request, PLAN_CALL) else {
        return Some(completed_tool_call(
            PLAN_CALL,
            "workflow",
            json!({"action": "plan", "file": ".rho/workflows/pty-background.star"}),
        ));
    };
    if !result.ok {
        return Some(completed(format!(
            "workflow fixture plan failed: {}",
            result.content
        )));
    }
    // Use the real frozen identity, not a seeded manifest or fixture runtime.
    let Some(plan_id) = result
        .content
        .lines()
        .find_map(|line| line.strip_prefix("plan_id: "))
    else {
        return Some(completed("workflow fixture plan returned no plan_id"));
    };
    Some(completed_tool_call(
        RUN_CALL,
        "workflow",
        json!({"action": "run", "plan_id": plan_id}),
    ))
}
