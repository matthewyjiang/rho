//! Host-owned transcript presentations shared by journals and rendering.

use serde::{Deserialize, Serialize};

/// A host transcript row has exactly one presentation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Presentation {
    Card(rho_tools::tool_card::ToolCard),
    Message(Box<MessageCard>),
}

impl From<rho_tools::tool_card::ToolCard> for Presentation {
    fn from(card: rho_tools::tool_card::ToolCard) -> Self {
        Self::Card(card)
    }
}

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
