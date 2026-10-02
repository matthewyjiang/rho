//! Rho-owned syntax metadata alongside the minor-compatible tools card.

use std::ops::Deref;

use rho_tools::tool_card::ToolCard;
use serde::{Deserialize, Serialize};

/// Which end of a source body stays visible when its card is collapsed.
/// The full body remains available on expansion; this never discards history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolBodyWindow {
    #[default]
    Head,
    Tail,
}

impl ToolBodyWindow {
    fn is_head(&self) -> bool {
        matches!(self, Self::Head)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ToolBodySyntax {
    #[default]
    Plain,
    Code {
        language: String,
        #[serde(default, skip_serializing_if = "ToolBodyWindow::is_head")]
        window: ToolBodyWindow,
    },
}

impl ToolBodySyntax {
    fn is_plain(&self) -> bool {
        matches!(self, Self::Plain)
    }
}

/// Keeps host-only rendering metadata out of the published `ToolCard` API.
/// Plain cards retain their existing attachment wire shape. Session replay
/// rebuilds syntax from call arguments instead of storing new session fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PresentedToolCard {
    #[serde(flatten)]
    pub(crate) card: ToolCard,
    #[serde(default, skip_serializing_if = "ToolBodySyntax::is_plain")]
    pub(crate) body_syntax: ToolBodySyntax,
}

impl From<ToolCard> for PresentedToolCard {
    fn from(card: ToolCard) -> Self {
        Self {
            card,
            body_syntax: ToolBodySyntax::Plain,
        }
    }
}

// Read-only access keeps existing card inspection call sites small. Mutations
// explicitly access `card` so they cannot discard the adjacent syntax metadata.
impl Deref for PresentedToolCard {
    type Target = ToolCard;

    fn deref(&self) -> &Self::Target {
        &self.card
    }
}

#[cfg(test)]
#[path = "interactive_presenter_tool_card_tests.rs"]
mod tests;
