//! How advisor mode reads on every surface that shows it.
//!
//! Advisor mode has three states, not two: a hand-edited config can leave the
//! mode on with no advisor model, and nothing reviews the session in that case.
//! Each surface renders the same three states so none of them claims the
//! advisor is working when it is not.

use crate::{agent::ADVISOR_AGENT_ID, config::InternalAgentModelConfig};

use super::RuntimeModelView;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AdvisorStatus {
    Off,
    /// Advisor mode is on and this model reviews the session.
    Reviewing {
        model: String,
    },
    /// Advisor mode is on with no advisor model, so the tool is not offered.
    MissingModel,
}

impl AdvisorStatus {
    pub(super) fn new(advisor_mode: bool, model: Option<&InternalAgentModelConfig>) -> Self {
        match (advisor_mode, model) {
            (false, _) => Self::Off,
            (true, Some(selection)) => Self::Reviewing {
                model: selection.display_reference(),
            },
            (true, None) => Self::MissingModel,
        }
    }

    pub(super) fn from_runtime(info: &RuntimeModelView) -> Self {
        Self::new(
            info.advisor_mode,
            info.internal_agents.get(ADVISOR_AGENT_ID),
        )
    }

    /// Short chrome label, or `None` while advisor mode is off. Off is the
    /// default, so it stays out of the composer divider.
    pub(super) fn indicator_text(&self) -> Option<String> {
        match self {
            Self::Off => None,
            Self::Reviewing { model } => Some(format!("advisor: {model}")),
            Self::MissingModel => Some("advisor: no model".into()),
        }
    }

    /// Config-picker badge for the advisor row.
    pub(super) fn badge(&self) -> String {
        match self {
            Self::Off => "off".into(),
            Self::Reviewing { model } => format!("on · {model}"),
            Self::MissingModel => "on · no model".into(),
        }
    }

    /// `/info` value for the advisor field.
    pub(super) fn detail(&self) -> String {
        match self {
            Self::Off => "off".into(),
            Self::Reviewing { model } => format!("on, {model} reviews the session"),
            Self::MissingModel => "on, but no advisor model is selected".into(),
        }
    }

    /// Whether the mode is on but cannot run for want of a model.
    pub(super) fn needs_model(&self) -> bool {
        matches!(self, Self::MissingModel)
    }
}

#[cfg(test)]
#[path = "advisor_status_tests.rs"]
mod tests;
