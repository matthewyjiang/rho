use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningLevel {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error(
    "invalid reasoning level '{0}'; expected one of: off, minimal, low, medium, high, xhigh, max"
)]
pub struct ParseReasoningLevelError(String);

impl ReasoningLevel {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Minimal,
            Self::Minimal => Self::Low,
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Xhigh,
            Self::Xhigh => Self::Max,
            Self::Max => Self::Off,
        }
    }

    pub fn next_for_model(self, provider: &str, model: &str) -> Self {
        let mut next = self.next();
        while !next.is_supported_by(provider, model) {
            next = next.next();
        }
        next
    }

    pub fn for_model(self, provider: &str, model: &str) -> Self {
        if self.is_supported_by(provider, model) {
            self
        } else {
            match self {
                Self::Max => Self::Xhigh,
                Self::Off | Self::Minimal | Self::Low | Self::Medium | Self::High | Self::Xhigh => {
                    self
                }
            }
        }
    }

    pub fn is_supported_by(self, provider: &str, model: &str) -> bool {
        match self {
            Self::Max => provider != "openai-codex" || codex_supports_max_effort(model),
            Self::Off | Self::Minimal | Self::Low | Self::Medium | Self::High | Self::Xhigh => true,
        }
    }

    pub fn effort(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Minimal => Some("low"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Xhigh => Some("xhigh"),
            Self::Max => Some("max"),
        }
    }

    pub fn summary(self) -> Option<&'static str> {
        self.effort().map(|_| "auto")
    }
}

fn codex_supports_max_effort(model: &str) -> bool {
    matches!(model, "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna")
}

impl fmt::Display for ReasoningLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        };
        formatter.write_str(value)
    }
}

impl FromStr for ReasoningLevel {
    type Err = ParseReasoningLevelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => Err(ParseReasoningLevelError(other.to_string())),
        }
    }
}

#[cfg(test)]
#[path = "reasoning_tests.rs"]
mod tests;
