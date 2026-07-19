use serde::{Deserialize, Deserializer, Serialize};

use crate::reasoning::ReasoningLevel;

/// A canonical finite set of reasoning levels advertised by a provider or catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReasoningLevelSet {
    levels: Vec<ReasoningLevel>,
}

impl ReasoningLevelSet {
    pub fn new(mut levels: Vec<ReasoningLevel>) -> Self {
        levels.sort_unstable();
        levels.dedup();
        Self { levels }
    }

    pub fn levels(&self) -> &[ReasoningLevel] {
        &self.levels
    }

    pub fn into_levels(self) -> Vec<ReasoningLevel> {
        self.levels
    }
}

impl<'de> Deserialize<'de> for ReasoningLevelSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SerializedLevelSet {
            levels: Vec<ReasoningLevel>,
        }

        let serialized = SerializedLevelSet::deserialize(deserializer)?;
        Ok(Self::new(serialized.levels))
    }
}

/// Describes whether reasoning support is unknown, unrestricted, or a finite set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningCapabilities {
    #[default]
    Unknown,
    Unrestricted,
    Levels(ReasoningLevelSet),
}

impl ReasoningCapabilities {
    pub fn from_metadata(levels: Option<Vec<ReasoningLevel>>, known: bool) -> Self {
        match (levels, known) {
            (Some(levels), _) => Self::Levels(ReasoningLevelSet::new(levels)),
            (None, true) => Self::Unrestricted,
            (None, false) => Self::Unknown,
        }
    }

    pub fn levels(&self) -> Option<&[ReasoningLevel]> {
        match self {
            Self::Levels(levels) => Some(levels.levels()),
            Self::Unknown | Self::Unrestricted => None,
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

#[cfg(test)]
#[path = "reasoning_capabilities_tests.rs"]
mod tests;
