use crate::{
    model::{ModelMetadata, ReasoningCapabilities, ReasoningRequestSource},
    reasoning::ReasoningLevel,
};

/// Immutable xAI reasoning behavior resolved once when the provider is built.
#[derive(Clone, Debug)]
pub(super) struct XaiReasoningProfile {
    capabilities: ReasoningCapabilities,
    offline_wire_behavior: OfflineWireBehavior,
}

#[derive(Clone, Copy, Debug)]
enum OfflineWireBehavior {
    Omit,
    Optional,
    Mandatory,
}

impl XaiReasoningProfile {
    pub(super) fn from_metadata(model: &str, metadata: Option<ModelMetadata>) -> Self {
        let offline_wire_behavior = match model {
            "grok-4.3" => OfflineWireBehavior::Optional,
            "grok-4.5" => OfflineWireBehavior::Mandatory,
            // These agent models do not accept the Responses API reasoning field.
            "grok-build-0.1" | "grok-composer-2.5-fast" => OfflineWireBehavior::Omit,
            // Do not guess the wire contract of newly introduced models.
            _ => OfflineWireBehavior::Omit,
        };
        Self {
            capabilities: metadata
                .map(|metadata| metadata.reasoning_capabilities())
                .unwrap_or_default(),
            offline_wire_behavior,
        }
    }

    #[cfg(test)]
    pub(super) fn exact(levels: impl IntoIterator<Item = ReasoningLevel>) -> Self {
        use crate::model::ReasoningLevelSet;

        Self {
            capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                levels.into_iter().collect(),
            )),
            offline_wire_behavior: OfflineWireBehavior::Omit,
        }
    }

    #[cfg(test)]
    pub(super) fn not_configurable() -> Self {
        Self {
            capabilities: ReasoningCapabilities::NotConfigurable,
            offline_wire_behavior: OfflineWireBehavior::Omit,
        }
    }

    pub(super) fn effort(&self, requested: ReasoningLevel) -> Option<&'static str> {
        match &self.capabilities {
            ReasoningCapabilities::NotConfigurable => None,
            ReasoningCapabilities::Unknown => match self.offline_wire_behavior {
                OfflineWireBehavior::Omit => None,
                OfflineWireBehavior::Optional => requested.effort().or(Some("none")),
                OfflineWireBehavior::Mandatory => requested.effort(),
            },
            ReasoningCapabilities::Levels(levels) => self
                .capabilities
                .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
                .effective()
                .and_then(|effective| {
                    if effective == ReasoningLevel::Off && levels.levels().contains(&effective) {
                        Some("none")
                    } else {
                        effective.effort()
                    }
                }),
        }
    }
}

#[cfg(test)]
#[path = "reasoning_tests.rs"]
mod tests;
