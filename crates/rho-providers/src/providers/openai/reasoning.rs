use crate::{
    model::{ModelError, ModelMetadata, ReasoningCapabilities, ReasoningRequestSource},
    reasoning::ReasoningLevel,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OpenAiReasoningConfig {
    pub(super) effort: Option<String>,
    pub(super) summary: Option<String>,
}

/// Immutable OpenAI reasoning controls resolved when the provider is built.
#[derive(Clone, Debug)]
pub(super) struct OpenAiReasoningProfile {
    metadata: Option<ModelMetadata>,
    capabilities: ReasoningCapabilities,
}

impl OpenAiReasoningProfile {
    pub(super) fn from_metadata(metadata: Option<ModelMetadata>) -> Self {
        let capabilities = metadata
            .as_ref()
            .map(ModelMetadata::reasoning_capabilities)
            .unwrap_or_default();
        Self {
            metadata,
            capabilities,
        }
    }

    #[cfg(test)]
    pub(super) fn unknown() -> Self {
        Self::from_metadata(None)
    }

    pub(super) fn config(
        &self,
        provider: &'static str,
        model: &str,
        requested: ReasoningLevel,
    ) -> Result<OpenAiReasoningConfig, ModelError> {
        if self.capabilities == ReasoningCapabilities::NotConfigurable {
            return Ok(OpenAiReasoningConfig {
                effort: None,
                summary: None,
            });
        }
        let Some(normalized_level) =
            normalize_openai_reasoning_level(requested, &self.capabilities)
        else {
            return Err(ModelError::UnsupportedReasoning {
                provider,
                model: model.to_string(),
                requested,
            });
        };
        let effort = self
            .metadata
            .as_ref()
            .and_then(|metadata| {
                metadata
                    .reasoning_effort(normalized_level)
                    .map(str::to_string)
            })
            .or_else(|| normalized_level.effort().map(str::to_string));
        Ok(OpenAiReasoningConfig {
            effort,
            summary: normalized_level.summary().map(str::to_string),
        })
    }
}

pub(super) fn normalize_openai_reasoning_level(
    requested: ReasoningLevel,
    capabilities: &ReasoningCapabilities,
) -> Option<ReasoningLevel> {
    match capabilities {
        ReasoningCapabilities::Unknown | ReasoningCapabilities::NotConfigurable => Some(requested),
        ReasoningCapabilities::Levels(levels) => {
            let supported = levels.levels();
            if supported.contains(&requested) {
                return Some(requested);
            }
            if requested == ReasoningLevel::Off {
                return capabilities
                    .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
                    .effective();
            }
            let configurable = supported
                .iter()
                .copied()
                .filter(|level| *level != ReasoningLevel::Off)
                .collect::<Vec<_>>();
            (!configurable.is_empty()).then(|| requested.normalize(Some(&configurable)))
        }
    }
}
