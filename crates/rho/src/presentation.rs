//! Host-owned transcript presentations shared by journals and rendering.

use serde::{Deserialize, Serialize};

pub(crate) mod checklist;
pub(crate) mod workflow;

/// A host transcript row has exactly one presentation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Presentation {
    Card(crate::app::interactive_presenter::PresentedToolCard),
    /// Facts-only receipt when collapsed; expansion reveals the full body.
    SummaryCard(crate::app::interactive_presenter::PresentedToolCard),
    /// Saved as `message`; the name predates non-agent notifications.
    #[serde(rename = "message")]
    Notification(Box<NotificationCard>),
}

impl From<rho_tools::tool_card::ToolCard> for Presentation {
    fn from(card: rho_tools::tool_card::ToolCard) -> Self {
        Self::Card(card.into())
    }
}

impl From<crate::app::interactive_presenter::PresentedToolCard> for Presentation {
    fn from(card: crate::app::interactive_presenter::PresentedToolCard) -> Self {
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
    /// Markdown shown in the card body.
    pub body: String,
    /// Verbatim text shown after `body`, such as a process's output tail.
    /// It is never read as markdown; see [`NotificationCard::markdown_body`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub details: Vec<String>,
}

impl NotificationCard {
    /// Body for markdown renderers: `body`, then `output` in a code fence that
    /// outgrows any backtick run inside it, so the output stays verbatim.
    pub(crate) fn markdown_body(&self) -> String {
        self.body_with_output(|output| {
            let longest_run = output
                .split(|ch| ch != '`')
                .map(str::len)
                .max()
                .unwrap_or(0);
            let fence = "`".repeat(longest_run.max(2) + 1);
            format!("{fence}text\n{output}\n{fence}")
        })
    }

    /// Body for hosts that show text as is: `body`, then `output` unfenced.
    pub(crate) fn plain_body(&self) -> String {
        self.body_with_output(str::to_owned)
    }

    fn body_with_output(&self, format_output: impl FnOnce(&str) -> String) -> String {
        match &self.output {
            Some(output) if self.body.is_empty() => format_output(output),
            Some(output) => format!("{}\n\n{}", self.body, format_output(output)),
            None => self.body.clone(),
        }
    }
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
        output: None,
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
