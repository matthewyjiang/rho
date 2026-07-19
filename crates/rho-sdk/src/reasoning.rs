use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
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
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

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

    pub fn next_supported(self, supported: Option<&[Self]>) -> Self {
        let Some(supported) = supported else {
            return self.next();
        };
        if supported.is_empty() {
            return self;
        }
        let mut next = self.next();
        for _ in 0..Self::ALL.len() {
            if supported.contains(&next) {
                return next;
            }
            next = next.next();
        }
        self
    }

    pub fn normalize(self, supported: Option<&[Self]>) -> Self {
        let Some(supported) = supported else {
            return self;
        };
        if supported.is_empty() {
            return self;
        }
        if supported.contains(&self) || self == Self::Off {
            return self;
        }
        supported
            .iter()
            .copied()
            .filter(|level| *level != Self::Off)
            .filter(|level| *level > self)
            .min()
            .or_else(|| {
                supported
                    .iter()
                    .copied()
                    .filter(|level| *level != Self::Off && *level < self)
                    .max()
            })
            .unwrap_or(self)
    }

    pub fn effort(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Minimal => Some("minimal"),
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
