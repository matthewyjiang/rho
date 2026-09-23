//! Host-owned transcript presentations shared by journals and rendering.

use serde::{Deserialize, Serialize};

/// A host transcript row has exactly one presentation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Presentation {
    Card(rho_tools::tool_card::ToolCard),
    /// Facts-only receipt when collapsed; expansion reveals the full body.
    SummaryCard(rho_tools::tool_card::ToolCard),
    /// Saved as `message`; the name predates non-agent notifications.
    #[serde(rename = "message")]
    Notification(Box<NotificationCard>),
}

impl From<rho_tools::tool_card::ToolCard> for Presentation {
    fn from(card: rho_tools::tool_card::ToolCard) -> Self {
        Self::Card(card)
    }
}

/// Something that arrived for the reader: an agent message, a delegated-run
/// result, or a finished background process. Sources supply the data; the
/// renderer stays source-agnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NotificationCard {
    pub title: String,
    pub sender: String,
    pub recipient: String,
    pub delivery: NotificationDelivery,
    #[serde(default)]
    pub tone: NotificationTone,
    #[serde(default)]
    pub preview: NotificationPreview,
    #[serde(default)]
    pub visibility: NotificationVisibility,
    /// Optional generic identity label displayed alongside routing information.
    #[serde(default)]
    pub reference: Option<String>,
    /// Replaces the sender/recipient routing line when routing is constant
    /// for the card's source (for example, background processes).
    #[serde(default)]
    pub subtitle: Option<String>,
    pub body: String,
    pub details: Vec<String>,
}

/// Incoming parent text, with delivery details supplied by the runtime owner.
pub(crate) fn parent_message_card(
    body: String,
    delivery: NotificationDelivery,
    detail: String,
) -> NotificationCard {
    NotificationCard {
        title: "Message from parent".into(),
        sender: "parent".into(),
        recipient: "agent".into(),
        delivery,
        tone: NotificationTone::Neutral,
        preview: NotificationPreview::Full,
        visibility: NotificationVisibility::Conversation,
        reference: None,
        subtitle: None,
        body,
        details: vec![detail],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotificationDelivery {
    Queued,
    Received,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotificationTone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotificationPreview {
    #[default]
    Truncated,
    Full,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotificationVisibility {
    #[default]
    Activity,
    Conversation,
}
