use crate::{
    model::{ModelMetadata, ReasoningCapabilities, ReasoningRequestSource},
    protocol::openai_chat::{ChatTemplateKwargs, OpenAiReasoning, OpenAiThinking},
    reasoning::ReasoningLevel,
};

use super::dialect::OpenAiCompatibleDialect;

#[derive(Default)]
pub(super) struct ReasoningFields {
    pub(super) reasoning: Option<OpenAiReasoning>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) thinking: Option<OpenAiThinking>,
    pub(super) chat_template_kwargs: Option<ChatTemplateKwargs>,
}

pub(super) struct OpenRouterReasoningProfile {
    capabilities: ReasoningCapabilities,
}

impl OpenRouterReasoningProfile {
    pub(super) fn from_metadata(metadata: Option<ModelMetadata>) -> Self {
        Self {
            capabilities: metadata
                .map(|metadata| metadata.reasoning_capabilities())
                .unwrap_or_default(),
        }
    }

    #[cfg(test)]
    pub(super) fn not_configurable() -> Self {
        Self {
            capabilities: ReasoningCapabilities::NotConfigurable,
        }
    }

    fn effort(&self, requested: ReasoningLevel) -> Option<&'static str> {
        match &self.capabilities {
            ReasoningCapabilities::NotConfigurable => None,
            ReasoningCapabilities::Unknown => Some(effort_or_none(requested)),
            ReasoningCapabilities::Levels(_) => self
                .capabilities
                .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
                .effective()
                .map(effort_or_none),
        }
    }
}

/// Immutable Moonshot reasoning controls resolved from exact catalog metadata.
#[derive(Clone, Debug)]
pub(super) struct MoonshotReasoningProfile {
    capabilities: ReasoningCapabilities,
    is_k3_wire_model: bool,
}

impl MoonshotReasoningProfile {
    pub(super) fn from_metadata(model: &str, metadata: Option<ModelMetadata>) -> Self {
        Self {
            capabilities: metadata
                .map(|metadata| metadata.reasoning_capabilities())
                .unwrap_or_default(),
            is_k3_wire_model: model == "kimi-k3",
        }
    }

    #[cfg(test)]
    pub(super) fn exact(levels: impl IntoIterator<Item = ReasoningLevel>) -> Self {
        use crate::model::ReasoningLevelSet;

        Self {
            capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                levels
                    .into_iter()
                    .filter(|level| *level != ReasoningLevel::Off)
                    .collect(),
            )),
            is_k3_wire_model: true,
        }
    }

    pub(super) fn effort(&self, requested: ReasoningLevel) -> Option<&'static str> {
        match &self.capabilities {
            ReasoningCapabilities::Unknown if self.is_k3_wire_model => requested.effort(),
            ReasoningCapabilities::Levels(_) => self
                .capabilities
                .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
                .effective()
                .and_then(ReasoningLevel::effort),
            ReasoningCapabilities::Unknown | ReasoningCapabilities::NotConfigurable => None,
        }
    }
}

pub(super) struct KimiReasoningProfile {
    capabilities: ReasoningCapabilities,
}

impl KimiReasoningProfile {
    pub(super) fn new(capabilities: ReasoningCapabilities) -> Self {
        Self { capabilities }
    }

    fn effective(&self, requested: ReasoningLevel) -> Option<ReasoningLevel> {
        match &self.capabilities {
            ReasoningCapabilities::NotConfigurable => None,
            ReasoningCapabilities::Unknown => Some(requested),
            ReasoningCapabilities::Levels(_) => self
                .capabilities
                .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
                .effective(),
        }
    }
}

impl OpenAiCompatibleDialect {
    pub(super) fn reasoning_fields(
        self,
        openrouter: Option<&OpenRouterReasoningProfile>,
        moonshot: Option<&MoonshotReasoningProfile>,
        kimi: Option<&KimiReasoningProfile>,
        model: &str,
        reasoning: ReasoningLevel,
    ) -> ReasoningFields {
        match self {
            Self::Standard => ReasoningFields::default(),
            Self::Poolside => ReasoningFields {
                chat_template_kwargs: (reasoning == ReasoningLevel::Off).then_some(
                    ChatTemplateKwargs {
                        enable_thinking: false,
                    },
                ),
                ..Default::default()
            },
            Self::OpenRouter => ReasoningFields {
                reasoning: openrouter
                    .and_then(|profile| profile.effort(reasoning))
                    .map(|effort| OpenAiReasoning {
                        effort: effort.to_string(),
                    }),
                ..Default::default()
            },
            Self::Moonshot => ReasoningFields {
                reasoning_effort: moonshot
                    .and_then(|profile| profile.effort(reasoning))
                    .map(str::to_string),
                ..Default::default()
            },
            Self::KimiCode => kimi_code_reasoning_fields(kimi, model, reasoning),
        }
    }
}

fn kimi_code_reasoning_fields(
    profile: Option<&KimiReasoningProfile>,
    model: &str,
    reasoning: ReasoningLevel,
) -> ReasoningFields {
    if model != "k3" {
        return Default::default();
    }
    let Some(reasoning) = profile.and_then(|profile| profile.effective(reasoning)) else {
        return Default::default();
    };
    ReasoningFields {
        thinking: Some(match reasoning {
            ReasoningLevel::Off => OpenAiThinking {
                kind: "disabled",
                effort: None,
            },
            ReasoningLevel::Minimal => enabled_thinking("minimal"),
            ReasoningLevel::Low => enabled_thinking("low"),
            ReasoningLevel::Medium => enabled_thinking("medium"),
            ReasoningLevel::High => enabled_thinking("high"),
            ReasoningLevel::Xhigh => enabled_thinking("xhigh"),
            ReasoningLevel::Max => enabled_thinking("max"),
        }),
        ..Default::default()
    }
}

fn effort_or_none(reasoning: ReasoningLevel) -> &'static str {
    reasoning.effort().unwrap_or("none")
}

fn enabled_thinking(effort: &str) -> OpenAiThinking {
    OpenAiThinking {
        kind: "enabled",
        effort: Some(effort.to_string()),
    }
}
