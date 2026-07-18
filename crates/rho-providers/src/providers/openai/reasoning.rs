use crate::{
    model::{models_dev, ModelError},
    reasoning::ReasoningLevel,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct OpenAiReasoningConfig {
    pub(super) effort: Option<String>,
    pub(super) summary: Option<String>,
}

pub(super) fn normalize_openai_reasoning_level(
    requested: ReasoningLevel,
    supported: Option<&[ReasoningLevel]>,
) -> Option<ReasoningLevel> {
    let Some(supported) = supported else {
        return Some(requested);
    };
    if requested == ReasoningLevel::Off {
        return Some(requested.normalize(Some(supported)));
    }
    let supported = supported
        .iter()
        .copied()
        .filter(|level| *level != ReasoningLevel::Off)
        .collect::<Vec<_>>();
    (!supported.is_empty()).then(|| requested.normalize(Some(&supported)))
}

pub(super) fn openai_reasoning_config(
    provider: &'static str,
    model: &str,
    requested: ReasoningLevel,
) -> Result<OpenAiReasoningConfig, ModelError> {
    let supported = models_dev::cached_reasoning_levels(provider, model);
    let Some(normalized_level) = normalize_openai_reasoning_level(requested, supported.as_deref())
    else {
        return Err(ModelError::UnsupportedReasoning {
            provider,
            model: model.to_string(),
            requested,
        });
    };
    Ok(OpenAiReasoningConfig {
        effort: models_dev::cached_reasoning_effort(provider, model, normalized_level),
        summary: normalized_level.summary().map(str::to_string),
    })
}
