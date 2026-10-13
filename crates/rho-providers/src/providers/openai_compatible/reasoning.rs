use crate::{
    model::{
        models_dev::{current_reasoning_metadata, ReasoningControl},
        ModelMetadata, ReasoningCapabilities, ReasoningLevelSet, ReasoningRequestSource,
    },
    protocol::openai_chat::{
        ChatTemplateKwargs, HiddenReasoningRisk, OpenAiReasoning, OpenAiThinking,
    },
    provider::UnknownEffortPolicy,
    reasoning::ReasoningLevel,
};

use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

#[cfg(test)]
#[path = "reasoning_catalog_tests.rs"]
mod catalog_tests;

/// Wire controls and accounting policy from the same per-request decision.
pub(super) struct ReasoningDecision {
    pub(super) reasoning: Option<OpenAiReasoning>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) thinking: Option<OpenAiThinking>,
    pub(super) chat_template_kwargs: Option<ChatTemplateKwargs>,
    pub(super) hidden_reasoning_risk: HiddenReasoningRisk,
}

impl Default for ReasoningDecision {
    fn default() -> Self {
        Self {
            reasoning: None,
            reasoning_effort: None,
            thinking: None,
            chat_template_kwargs: None,
            hidden_reasoning_risk: HiddenReasoningRisk::Unlikely,
        }
    }
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
            OpenAiCompatibleDialect::Standard
            | OpenAiCompatibleDialect::Custom
            | OpenAiCompatibleDialect::QwenTokenPlan => {
                let (metadata, control) = current_reasoning_metadata(provider, model);
                Self::Standard(standard_effort_profile(provider, metadata).with_control(control))
            }
            OpenAiCompatibleDialect::Poolside => Self::Poolside,
            OpenAiCompatibleDialect::OpenRouter => {
                let (metadata, control) = current_reasoning_metadata(provider, model);
                Self::OpenRouter(EffortProfile::send_when_unknown(metadata).with_control(control))
            }
            OpenAiCompatibleDialect::Moonshot => {
                Self::Moonshot(MoonshotReasoningProfile::from_metadata(model, metadata()))
            }
            OpenAiCompatibleDialect::KimiCode => Self::KimiCode(KimiReasoningProfile::new(
                crate::model::models_dev::current_reasoning_capabilities(provider, model),
            )),
        }
    }

    pub(super) fn resolve(&self, model: &str, reasoning: ReasoningLevel) -> ReasoningDecision {
        match self {
            Self::Standard(profile) => {
                let resolved = profile.resolve(reasoning);
                ReasoningDecision {
                    reasoning_effort: resolved.effort().map(str::to_string),
                    hidden_reasoning_risk: resolved.hidden_reasoning_risk(),
                    ..Default::default()
                }
            }
            Self::Poolside => ReasoningDecision {
                chat_template_kwargs: (reasoning == ReasoningLevel::Off).then_some(
                    ChatTemplateKwargs {
                        enable_thinking: false,
                    },
                ),
                // Poolside enables thinking by omission for every non-Off request.
                hidden_reasoning_risk: if reasoning == ReasoningLevel::Off {
                    HiddenReasoningRisk::Unlikely
                } else {
                    HiddenReasoningRisk::Possible
                },
                ..Default::default()
            },
            Self::OpenRouter(profile) => {
                let resolved = profile.resolve(reasoning);
                ReasoningDecision {
                    reasoning: match resolved {
                        ResolvedEffort::Omitted => None,
                        ResolvedEffort::ToggleOn => {
                            Some(OpenAiReasoning::Enabled { enabled: true })
                        }
                        ResolvedEffort::Named(level) => Some(OpenAiReasoning::Effort {
                            effort: effort_or_none(level).to_string(),
                        }),
                    },
                    hidden_reasoning_risk: resolved.hidden_reasoning_risk(),
                    ..Default::default()
                }
            }
            Self::Moonshot(profile) => {
                let effort = profile.effort(reasoning);
                ReasoningDecision {
                    reasoning_effort: effort.map(str::to_string),
                    // Moonshot's profile only returns enabled effort levels.
                    hidden_reasoning_risk: if effort.is_some() {
                        HiddenReasoningRisk::Possible
                    } else {
                        HiddenReasoningRisk::Unlikely
                    },
                    ..Default::default()
                }
            }
            Self::KimiCode(profile) => kimi_code_reasoning_decision(profile, model, reasoning),
        }
    }
}

/// Standard-dialect hosts take unknown-model effort from the descriptor.
/// Ollama constrains to the server's vocabulary; config-defined hosts send the
/// requested level so thinking can be turned off.
fn standard_effort_profile(
    provider: &'static str,
    metadata: Option<ModelMetadata>,
) -> EffortProfile {
    let Some(descriptor) = crate::provider::provider_descriptor(provider)
        .or_else(|| crate::provider::interned_custom_openai_compatible_provider(provider))
    else {
        return EffortProfile::omit_when_unknown(metadata);
    };
    match descriptor.unknown_effort() {
        UnknownEffortPolicy::Omit => EffortProfile::omit_when_unknown(metadata),
        UnknownEffortPolicy::SendRequested => EffortProfile::send_when_unknown(metadata),
        UnknownEffortPolicy::Constrain(levels) if metadata.is_none() => {
            EffortProfile::constrained(levels.iter().copied())
        }
        UnknownEffortPolicy::Constrain(_) => EffortProfile::send_when_unknown(metadata),
    }
}

/// Metadata-driven effort selection for dialects that speak level names on the
/// wire, with an explicit policy for models whose capabilities are unknown.
pub(super) struct EffortProfile {
    capabilities: ReasoningCapabilities,
    when_unknown: UnknownCapabilities,
    control: ReasoningControl,
}

/// What to serialize when a model's reasoning capabilities are unknown.
enum UnknownCapabilities {
    /// Omit the field so the host applies its own default.
    Omit,
    /// Send the requested level unchanged.
    SendRequested,
}

/// Preserve toggle-on intent even when the dialect enables it without an effort.
#[derive(Clone, Copy)]
enum ResolvedEffort {
    Omitted,
    ToggleOn,
    Named(ReasoningLevel),
}

impl ResolvedEffort {
    fn effort(self) -> Option<&'static str> {
        match self {
            Self::Omitted | Self::ToggleOn => None,
            Self::Named(level) => Some(effort_or_none(level)),
        }
    }

    fn hidden_reasoning_risk(self) -> HiddenReasoningRisk {
        match self {
            Self::Omitted | Self::Named(ReasoningLevel::Off) => HiddenReasoningRisk::Unlikely,
            Self::ToggleOn | Self::Named(_) => HiddenReasoningRisk::Possible,
        }
    }
}

impl EffortProfile {
    fn with_control(mut self, control: ReasoningControl) -> Self {
        self.control = control;
        self
    }

    pub(super) fn omit_when_unknown(metadata: Option<ModelMetadata>) -> Self {
        Self::from_metadata(metadata, UnknownCapabilities::Omit)
    }

    pub(super) fn send_when_unknown(metadata: Option<ModelMetadata>) -> Self {
        Self::from_metadata(metadata, UnknownCapabilities::SendRequested)
    }

    fn constrained(levels: impl IntoIterator<Item = ReasoningLevel>) -> Self {
        Self {
            capabilities: ReasoningCapabilities::Levels(ReasoningLevelSet::new(
                levels.into_iter().collect(),
            )),
            when_unknown: UnknownCapabilities::Omit,
            control: ReasoningControl::Effort,
        }
    }

    fn from_metadata(metadata: Option<ModelMetadata>, when_unknown: UnknownCapabilities) -> Self {
        Self {
            capabilities: metadata
                .map(|metadata| metadata.reasoning_capabilities())
                .unwrap_or_default(),
            when_unknown,
            control: ReasoningControl::Effort,
        }
    }

    #[cfg(test)]
    pub(super) fn levels(levels: impl IntoIterator<Item = ReasoningLevel>) -> Self {
        Self::constrained(levels)
    }

    #[cfg(test)]
    pub(super) fn not_configurable() -> Self {
        Self {
            capabilities: ReasoningCapabilities::NotConfigurable,
            when_unknown: UnknownCapabilities::Omit,
            control: ReasoningControl::Effort,
        }
    }

    fn resolve(&self, requested: ReasoningLevel) -> ResolvedEffort {
        let Some(effective) = self.effective(requested) else {
            return ResolvedEffort::Omitted;
        };
        match (self.control, effective) {
            // Toggle-on has no named effort; each dialect enables its default.
            (ReasoningControl::Toggle, ReasoningLevel::Off) => {
                ResolvedEffort::Named(ReasoningLevel::Off)
            }
            (ReasoningControl::Toggle, _) => ResolvedEffort::ToggleOn,
            (ReasoningControl::Effort, _) => ResolvedEffort::Named(effective),
        }
    }

    fn effective(&self, requested: ReasoningLevel) -> Option<ReasoningLevel> {
        match &self.capabilities {
            ReasoningCapabilities::NotConfigurable => None,
            ReasoningCapabilities::Unknown => match self.when_unknown {
                UnknownCapabilities::Omit => None,
                UnknownCapabilities::SendRequested => Some(requested),
            },
            ReasoningCapabilities::Levels(_) => self
                .capabilities
                .resolve(requested, ReasoningRequestSource::PersistedOrDefault)
                .effective(),
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

fn kimi_code_reasoning_decision(
    profile: &KimiReasoningProfile,
    model: &str,
    reasoning: ReasoningLevel,
) -> ReasoningDecision {
    if model != "k3" {
        return Default::default();
    }
    let Some(reasoning) = profile.effective(reasoning) else {
        return Default::default();
    };
    ReasoningDecision {
        hidden_reasoning_risk: if reasoning == ReasoningLevel::Off {
            HiddenReasoningRisk::Unlikely
        } else {
            HiddenReasoningRisk::Possible
        },
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
