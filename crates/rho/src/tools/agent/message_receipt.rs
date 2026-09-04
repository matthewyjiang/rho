//! Durable task identity on a parent-to-child message receipt.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct MessageReceipt {
    pub run_id: String,
    pub agent_id: String,
    pub task: String,
}

impl MessageReceipt {
    pub(crate) fn content(&self) -> String {
        format!(
            "queued parent message for delegated run '{}'\n{}",
            self.run_id,
            serde_json::to_string(self).expect("message receipt contains only strings")
        )
    }

    pub(crate) fn parse(content: &str) -> Option<Self> {
        let (receipt, identity) = content.split_once('\n')?;
        receipt
            .starts_with("queued parent message for delegated run '")
            .then_some(())?;
        serde_json::from_str(identity).ok()
    }
}
