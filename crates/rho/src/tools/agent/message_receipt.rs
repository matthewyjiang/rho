//! Durable task identity on a parent-to-child message receipt.

use serde::Deserialize;

#[derive(Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct MessageReceipt {
    pub run_id: String,
    pub agent_id: String,
    pub task: String,
}

impl MessageReceipt {
    pub(crate) fn content(&self) -> String {
        format!(
            "queued parent message for delegated run '{}' to {}\nTask: {}",
            self.run_id, self.agent_id, self.task
        )
    }

    pub(crate) fn parse(content: &str) -> Option<Self> {
        let (header, body) = content.split_once('\n')?;
        let identity = header.strip_prefix("queued parent message for delegated run '")?;
        if let Some((run_id, agent_id)) = identity.split_once("' to ") {
            // Only the first header line has structure. The remaining task may
            // contain newlines or any of these delimiters without ambiguity.
            return Some(Self {
                run_id: run_id.into(),
                agent_id: agent_id.into(),
                task: body.strip_prefix("Task: ")?.into(),
            });
        }
        // Retain replay of receipts saved by the earlier JSON representation.
        let run_id = identity.strip_suffix('\'')?;
        let receipt: Self = serde_json::from_str(body).ok()?;
        (receipt.run_id == run_id).then_some(receipt)
    }
}

#[cfg(test)]
#[path = "message_receipt_tests.rs"]
mod tests;
