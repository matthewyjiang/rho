//! Keep the proposed plan available when the approval receipt is expanded.

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFamily, ToolHeader, ToolStatus};

pub(super) fn card(arguments: &serde_json::Value, status: ToolStatus) -> ToolCard {
    let mut card = ToolCard::new(
        status,
        ToolFamily::Form,
        ToolHeader::call("exit_plan_mode", Some("Plan approval".into())),
    );
    if let Some(plan) = arguments.get("plan").and_then(serde_json::Value::as_str) {
        card.body = ToolBody::Lines(super::split_body_lines(plan));
    }
    card
}
