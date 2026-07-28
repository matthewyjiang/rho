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

/// Per-dialect reasoning policy, paired with the profile that dialect needs.
///
/// Construction is keyed by [`OpenAiCompatibleDialect`], so a provider can
/// never hold a profile that does not match its dialect.
pub(super) enum DialectReasoning {
    /// Metadata-driven `reasoning_effort` for Ollama Cloud and other Standard
    /// dialect hosts. Ollama auto-enables thinking when the field is omitted,
    /// so Off must serialize as `"none"`.
    Standard(EffortProfile),
    Poolside,
    OpenRouter(EffortProfile),
    Moonshot(MoonshotReasoningProfile),
    KimiCode(KimiReasoningProfile),
}

impl DialectReasoning {
    pub(super) fn new(
        dialect: OpenAiCompatibleDialect,
        provider: &'static str,
        model: &str,
    ) -> Self {
        let metadata = || crate::model::models_dev::current_model_metadata(provider, model);
        match dialect {
            OpenAiCompatibleDialect::Standard => {
                Self::Standard(EffortProfile::omit_when_unknown(metadata()))
            }
            OpenAiCompatibleDialect::Poolside => Self::Poolside,
            OpenAiCompatibleDialect::OpenRouter => {
                Self::OpenRouter(EffortProfile::send_when_unknown(metadata()))
            }
            OpenAiCompatibleDialect::Moonshot => {
                Self::Moonshot(MoonshotReasoningProfile::from_metadata(model, metadata()))
            }
            OpenAiCompatibleDialect::KimiCode => Self::KimiCode(KimiReasoningProfile::new(
                crate::model::models_dev::current_reasoning_capabilities(provider, model),
            )),
        }
    }

    pub(super) fn fields(&self, model: &str, reasoning: ReasoningLevel) -> ReasoningFields {
        match self {
            Self::Standard(profile) => ReasoningFields {
                reasoning_effort: profile.effort(reasoning).map(str::to_string),
                ..Default::default()
            },
            Self::Poolside => ReasoningFields {
                chat_template_kwargs: (reasoning == ReasoningLevel::Off).then_some(
                    ChatTemplateKwargs {
                        enable_thinking: false,
                    },
                ),
                ..Default::default()
            },
            Self::OpenRouter(profile) => ReasoningFields {
                reasoning: profile.effort(reasoning).map(|effort| OpenAiReasoning {
                    effort: effort.to_string(),
                }),
                ..Default::default()
            },
            Self::Moonshot(profile) => ReasoningFields {
                reasoning_effort: profile.effort(reasoning).map(str::to_string),
                ..Default::default()
            },
            Self::KimiCode(profile) => kimi_code_reasoning_fields(profile, model, reasoning),
        }
    }
}

/// Metadata-driven effort selection for dialects that speak level names on the
/// wire, with an explicit policy for models whose capabilities are unknown.
pub(super) struct EffortProfile {
    capabilities: ReasoningCapabilities,
    when_unknown: UnknownCapabilities,
}

/// What to serialize when a model's reasoning capabilities are unknown.
enum UnknownCapabilities {
    /// Omit the field so the host applies its own default.
    Omit,
    /// Send the requested level unchanged.
    SendRequested,
}

impl EffortProfile {
    pub(super) fn omit_when_unknown(metadata: Option<ModelMetadata>) -> Self {
        Self::from_metadata(metadata, UnknownCapabilities::Omit)
    }

    pub(super) fn send_when_unknown(metadata: Option<ModelMetadata>) -> Self {
        Self::from_metadata(metadata, UnknownCapabilities::SendRequested)
    }

    fn from_metadata(metadata: Option<ModelMetadata>, when_unknown: UnknownCapabilities) -> Self {
        Self {
            capabilities: metadata
                .map(|metadata| metadata.reasoning_capabilities())
                .unwrap_or_default(),
            when_unknown,
        }
    }

    #[cfg(test)]
    pub(super) fn levels(levels: impl IntoIterator<Item = ReasoningLevel>) -> Self {
        use crate::model::ReasoningLevelSet;

        Self {
            capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                levels.into_iter().collect(),
            )),
            when_unknown: UnknownCapabilities::Omit,
        }
    }

    #[cfg(test)]
    pub(super) fn not_configurable() -> Self {
        Self {
            capabilities: ReasoningCapabilities::NotConfigurable,
            when_unknown: UnknownCapabilities::Omit,
        }
    }

    fn effort(&self, requested: ReasoningLevel) -> Option<&'static str> {
        match &self.capabilities {
            ReasoningCapabilities::NotConfigurable => None,
            ReasoningCapabilities::Unknown => match self.when_unknown {
                UnknownCapabilities::Omit => None,
                UnknownCapabilities::SendRequested => Some(effort_or_none(requested)),
            },
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

fn kimi_code_reasoning_fields(
    profile: &KimiReasoningProfile,
    model: &str,
    reasoning: ReasoningLevel,
) -> ReasoningFields {
    if model != "k3" {
        return Default::default();
    }
    let Some(reasoning) = profile.effective(reasoning) else {
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
