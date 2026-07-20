use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

use crate::reasoning::ReasoningLevel;

/// A canonical finite set of reasoning levels advertised by a provider or catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReasoningLevelSet {
    levels: Vec<ReasoningLevel>,
}

impl ReasoningLevelSet {
    pub fn new(mut levels: Vec<ReasoningLevel>) -> Self {
        assert!(
            !levels.is_empty(),
            "reasoning level sets must contain at least one level"
        );
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
        if serialized.levels.is_empty() {
            return Err(D::Error::custom(
                "reasoning level sets must contain at least one level",
            ));
        }
        Ok(Self::new(serialized.levels))
    }
}

/// Describes the exact user-configurable reasoning controls for a model.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningCapabilities {
    /// Capability metadata is unavailable or incomplete.
    #[default]
    #[serde(alias = "unrestricted")]
    Unknown,
    /// The model has no user-selectable reasoning level.
    NotConfigurable,
    /// The model accepts exactly these levels. `Off` is present only when selectable.
    Levels(ReasoningLevelSet),
}

/// Identifies whether an unsupported request may be normalized automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasoningRequestSource {
    /// A user explicitly selected this value for the current invocation.
    Explicit,
    /// The value came from persisted configuration or an application default.
    PersistedOrDefault,
}

/// Result of resolving a requested level against model capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasoningResolution {
    Exact(ReasoningLevel),
    Normalized {
        requested: ReasoningLevel,
        effective: ReasoningLevel,
    },
    UnsupportedExplicit(ReasoningLevel),
    NotConfigurable,
    Unknown(ReasoningLevel),
}

impl ReasoningResolution {
    /// Returns an effective level when applying the result is safe.
    pub fn effective(self) -> Option<ReasoningLevel> {
        match self {
            Self::Exact(level) | Self::Unknown(level) => Some(level),
            Self::Normalized { effective, .. } => Some(effective),
            Self::UnsupportedExplicit(_) | Self::NotConfigurable => None,
        }
    }
}

impl ReasoningCapabilities {
    pub fn from_metadata(levels: Option<Vec<ReasoningLevel>>, known: bool) -> Self {
        match (levels, known) {
            (Some(levels), _) if levels.is_empty() => Self::NotConfigurable,
            (Some(levels), _) => Self::Levels(ReasoningLevelSet::new(levels)),
            (None, true) => Self::NotConfigurable,
            (None, false) => Self::Unknown,
        }
    }

    pub fn levels(&self) -> Option<&[ReasoningLevel]> {
        match self {
            Self::Levels(levels) => Some(levels.levels()),
            Self::Unknown | Self::NotConfigurable => None,
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// Resolves a request without treating `Off` as implicitly supported.
    pub fn resolve(
        &self,
        requested: ReasoningLevel,
        source: ReasoningRequestSource,
    ) -> ReasoningResolution {
        let effective = match self {
            Self::Unknown => return ReasoningResolution::Unknown(requested),
            Self::NotConfigurable => return ReasoningResolution::NotConfigurable,
            Self::Levels(levels) if levels.levels().contains(&requested) => {
                return ReasoningResolution::Exact(requested);
            }
            Self::Levels(levels) => nearest_level(requested, levels.levels()),
        };

        if source == ReasoningRequestSource::Explicit {
            ReasoningResolution::UnsupportedExplicit(requested)
        } else {
            ReasoningResolution::Normalized {
                requested,
                effective,
            }
        }
    }

    /// Advances through only selectable levels. Unknown capabilities retain the global cycle.
    pub fn next_level(&self, current: ReasoningLevel) -> ReasoningLevel {
        match self {
            Self::Unknown => current.next(),
            Self::NotConfigurable => current,
            Self::Levels(levels) => current.next_supported(Some(levels.levels())),
        }
    }
}

fn nearest_level(requested: ReasoningLevel, levels: &[ReasoningLevel]) -> ReasoningLevel {
    levels
        .iter()
        .copied()
        .filter(|level| *level > requested)
        .min()
        .or_else(|| {
            levels
                .iter()
                .copied()
                .filter(|level| *level < requested)
                .max()
        })
        .unwrap_or(ReasoningLevel::Off)
}

#[cfg(test)]
#[path = "reasoning_capabilities_tests.rs"]
mod tests;
