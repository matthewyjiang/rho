//! Host-owned message presentation, independent of tool execution status.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MessageCard {
    pub title: String,
    pub sender: String,
    pub recipient: String,
    pub delivery: MessageDelivery,
    pub body: String,
    pub details: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessageDelivery {
    Queued,
}
