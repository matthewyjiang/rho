//! How Rho draws a tool call: the cards of its interactive transcript.
//!
//! The daemon sends them on [`crate::ToolCallUpdate::display`], so every
//! frontend can draw the cards Rho's TUI draws. Run journals and saved
//! sessions store the same types, so their serde shapes only ever grow in
//! ways old records still read.

mod card;
mod notification;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use card::{
    CodeWindow, DiffRow, DiffRowKind, ToolBody, ToolCard, ToolCardStatus, ToolFact, ToolFamily,
    ToolHeader,
};
pub use notification::{
    NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
    NotificationVisibility,
};

/// A transcript row has exactly one presentation.
///
/// Closed: each variant carries its card under its own key, which serde
/// cannot read into a unit `Unknown`, so a new variant is a protocol major.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    Card(ToolCard),
    /// Facts-only receipt when collapsed; expansion reveals the full body.
    SummaryCard(ToolCard),
    /// Saved as `message`; the name predates non-agent notifications.
    #[serde(rename = "message")]
    Notification(Box<NotificationCard>),
}

impl From<ToolCard> for Presentation {
    fn from(card: ToolCard) -> Self {
        Self::Card(card)
    }
}
