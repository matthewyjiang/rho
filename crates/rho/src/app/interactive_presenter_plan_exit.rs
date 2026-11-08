//! Keep the proposed plan available when the approval receipt is expanded.

use rho_protocol::display::{ToolBody, ToolCard, ToolCardStatus, ToolFamily, ToolHeader};

pub(super) fn card(arguments: &serde_json::Value, status: ToolCardStatus) -> ToolCard {
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
