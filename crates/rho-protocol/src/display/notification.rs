//! Notification cards: something that arrived for the reader rather than a
//! tool's own result.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Something that arrived for the reader: an agent message, a delegated-run
/// result, or a finished background process. Sources supply the data; the
/// renderer stays source-agnostic.
///
/// Fields added after the first saved journals read as their defaults when
/// absent, and are left out when they hold them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationCard {
    pub title: String,
    pub sender: String,
    pub recipient: String,
    pub delivery: NotificationDelivery,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tone: NotificationTone,
    #[serde(default, skip_serializing_if = "is_default")]
    pub preview: NotificationPreview,
    #[serde(default, skip_serializing_if = "is_default")]
    pub visibility: NotificationVisibility,
    /// Optional generic identity label displayed alongside routing information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Replaces the sender/recipient routing line when routing is constant
    /// for the card's source (for example, background processes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Markdown shown in the card body.
    pub body: String,
    /// Verbatim text shown after `body`, such as a process's output tail.
    /// It is never read as markdown; see [`NotificationCard::markdown_body`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub details: Vec<String>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

impl NotificationCard {
    /// Body for markdown renderers: `body`, then `output` in a code fence that
    /// outgrows any backtick run inside it, so the output stays verbatim.
    pub fn markdown_body(&self) -> String {
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
    pub fn plain_body(&self) -> String {
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

/// Whether the card's content waits for the recipient or has reached it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotificationDelivery {
    Queued,
    Received,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How a notification card reads, which picks its accent color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotificationTone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Error,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How much of the body a collapsed card shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotificationPreview {
    #[default]
    Truncated,
    Full,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Whether the card belongs to the conversation or only to the activity
/// around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotificationVisibility {
    #[default]
    Activity,
    Conversation,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
