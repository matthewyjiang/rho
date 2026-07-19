use rho_providers::{
    model::{
        models_dev, ModelMetadata, ReasoningCapabilities, ReasoningRequestSource,
        ReasoningResolution,
    },
    reasoning::ReasoningLevel,
};

pub(super) struct FetchedReasoningResolution {
    pub(super) effective: ReasoningLevel,
    pub(super) rejected: Option<ReasoningLevel>,
}

impl super::TuiInfo {
    pub(super) fn set_reasoning(&mut self, level: ReasoningLevel, source: ReasoningRequestSource) {
        self.reasoning = level;
        self.reasoning_source = source;
        self.diagnostics
            .update_identity(&self.provider, &self.model, level);
    }
}

pub(super) fn cached_metadata(provider: &str, model: &str) -> Option<(ModelMetadata, bool)> {
    let metadata = models_dev::cached_model_metadata(provider, model)?;
    let is_current = !models_dev::model_metadata_needs_refresh(provider, model);
    Some((metadata, is_current))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelSwitchReasoningResolution {
    pub(super) effective: ReasoningLevel,
    pub(super) source: ReasoningRequestSource,
}

pub(super) fn resolve_model_switch_reasoning(
    capabilities: &ReasoningCapabilities,
    requested: ReasoningLevel,
    source: ReasoningRequestSource,
) -> Result<ModelSwitchReasoningResolution, ReasoningLevel> {
    let resolution = capabilities.resolve(requested, source);
    match resolution {
        ReasoningResolution::UnsupportedExplicit(requested) => Err(requested),
        ReasoningResolution::Normalized { effective, .. } => Ok(ModelSwitchReasoningResolution {
            effective,
            source: ReasoningRequestSource::PersistedOrDefault,
        }),
        ReasoningResolution::Exact(effective) | ReasoningResolution::Unknown(effective) => {
            Ok(ModelSwitchReasoningResolution { effective, source })
        }
        ReasoningResolution::NotConfigurable => Ok(ModelSwitchReasoningResolution {
            effective: requested,
            source,
        }),
    }
}

pub(super) fn resolve_fetched_reasoning(
    capabilities: &ReasoningCapabilities,
    current: ReasoningLevel,
    at_fetch_start: Option<(ReasoningLevel, ReasoningRequestSource)>,
) -> FetchedReasoningResolution {
    let source = match at_fetch_start {
        Some((reasoning, _)) if reasoning != current => ReasoningRequestSource::Explicit,
        Some((_, source)) => source,
        None => ReasoningRequestSource::PersistedOrDefault,
    };
    let resolution = capabilities.resolve(current, source);
    if let ReasoningResolution::UnsupportedExplicit(requested) = resolution {
        let effective = at_fetch_start
            .and_then(|(reasoning, _)| {
                capabilities
                    .resolve(reasoning, ReasoningRequestSource::PersistedOrDefault)
                    .effective()
            })
            .unwrap_or(current);
        return FetchedReasoningResolution {
            effective,
            rejected: Some(requested),
        };
    }
    FetchedReasoningResolution {
        effective: resolution.effective().unwrap_or(current),
        rejected: None,
    }
}

#[cfg(test)]
#[path = "reasoning_metadata_tests.rs"]
mod tests;
