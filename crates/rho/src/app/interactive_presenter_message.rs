//! Delegation policy for the generic host message card.

use super::{ToolKind, ToolView};
use crate::{
    presentation::{MessageCard, MessageDelivery},
    tools::agent::message_receipt::MessageReceipt,
};

pub(super) fn finished_message(
    view: &ToolView,
    content: &str,
    ok: bool,
) -> Option<Box<MessageCard>> {
    if !ok || view.kind != ToolKind::Agents || view.arguments.get("action")?.as_str()? != "message"
    {
        return None;
    }
    let id = view.arguments.get("id")?.as_str()?;
    let receipt = MessageReceipt::parse(content);
    let (title, recipient, run_id) = match receipt {
        Some(receipt) => (receipt.task, receipt.agent_id, receipt.run_id),
        None => ("Delegated task".into(), "child".into(), id.into()),
    };
    Some(Box::new(MessageCard {
        title,
        sender: "parent".into(),
        recipient,
        delivery: MessageDelivery::Queued,
        body: view.arguments.get("message")?.as_str()?.trim().into(),
        details: vec![
            format!("run: {run_id}"),
            format!("attach: rho attach {run_id}"),
        ],
    }))
}
