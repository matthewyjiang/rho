use std::{borrow::Cow, collections::BTreeMap, path::PathBuf};

use serde::Serialize;

use {
    crate::keybindings::Keybindings, crate::model_aliases::ModelAliases,
    crate::permission::PermissionMode, rho_providers::credentials::CredentialStoreBackend,
    rho_providers::reasoning::ReasoningLevel,
};

use super::{provider_config::PersistedProviderConfigs, Config, SearchProvider};

pub(super) fn write_config(path: &PathBuf, config: &Config) -> anyhow::Result<()> {
    let serialized = toml::to_string_pretty(&GroupedConfig::from(config))?;
    crate::config_writer::write_atomically(path, &serialized)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternalAgentModelConfig {
    pub provider: String,
    pub model: String,
    pub auth: String,
    pub(super) model_alias: Option<String>,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveModelConfig {
    pub provider: String,
    pub model: String,
    pub auth: String,
    pub source: EffectiveModelSource,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveModelSource {
    Conversation,
    Override,
}

impl InternalAgentModelConfig {
    pub fn new(provider: String, model: String, auth: String) -> Self {
        Self {
            provider,
            model,
            auth,
            model_alias: None,
        }
    }

    pub(super) fn current_alias<'a>(&'a self, aliases: &'a ModelAliases) -> Option<&'a str> {
        let name = self.model_alias.as_deref()?;
        let target = aliases.get(name)?;
        (target.model == self.model
            && target.provider.as_deref().unwrap_or(&self.provider) == self.provider)
            .then_some(name)
    }
}

#[derive(Serialize)]
struct GroupedConfig<'a> {
    model: ModelConfig<'a>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    internal_agents: BTreeMap<&'a str, PersistedInternalAgentModelConfig<'a>>,
    display: DisplayConfig,
    output: OutputConfig,
    compaction: CompactionSection,
    web_search: WebSearchConfig<'a>,
    behavior: BehaviorConfig<'a>,
    keybindings: &'a Keybindings,
    prompt_templates: &'a crate::prompt_templates::PromptTemplates,
    providers: PersistedProviderConfigs<'a>,
}

#[derive(Serialize)]
struct ModelConfig<'a> {
    provider: &'a str,
    model: Cow<'a, str>,
    auth: &'a str,
    reasoning: ReasoningLevel,
    favorite_models: &'a [String],
    #[serde(skip_serializing_if = "ModelAliases::is_empty")]
    aliases: &'a ModelAliases,
}

#[derive(Serialize)]
struct DisplayConfig {
    show_reasoning_output: bool,
    max_tool_output_lines: usize,
}

#[derive(Serialize)]
struct OutputConfig {
    max_output_bytes: usize,
}

#[derive(Serialize)]
struct CompactionSection {
    auto_compact: bool,
    compact_threshold_percent: u8,
    compact_target_percent: u8,
}

#[derive(Serialize)]
struct PersistedInternalAgentModelConfig<'a> {
    provider: &'a str,
    model: Cow<'a, str>,
    auth: &'a str,
}

#[derive(Serialize)]
struct WebSearchConfig<'a> {
    provider: SearchProvider,
    #[serde(skip_serializing_if = "Option::is_none")]
    openai_api_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exa_api_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    brave_api_key: Option<&'a str>,
}

#[derive(Serialize)]
struct BehaviorConfig<'a> {
    check_for_updates: bool,
    enable_subagents: bool,
    permission_mode: PermissionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_store: Option<&'a str>,
    rtk: bool,
    inline_shell: &'a str,
}

impl<'a> From<&'a Config> for GroupedConfig<'a> {
    fn from(config: &'a Config) -> Self {
        Self {
            model: ModelConfig {
                provider: &config.provider,
                model: persisted_model_reference(config.current_model_alias(), &config.model),
                auth: &config.auth,
                reasoning: config.reasoning,
                favorite_models: &config.favorite_models,
                aliases: &config.model_aliases,
            },
            internal_agents: config
                .internal_agents
                .iter()
                .map(|(id, selection)| {
                    (
                        id.as_str(),
                        PersistedInternalAgentModelConfig {
                            provider: &selection.provider,
                            model: persisted_model_reference(
                                selection.current_alias(&config.model_aliases),
                                &selection.model,
                            ),
                            auth: &selection.auth,
                        },
                    )
                })
                .collect(),
            display: DisplayConfig {
                show_reasoning_output: config.show_reasoning_output,
                max_tool_output_lines: config.max_tool_output_lines,
            },
            output: OutputConfig {
                max_output_bytes: config.max_output_bytes,
            },
            compaction: CompactionSection {
                auto_compact: config.auto_compact,
                compact_threshold_percent: config.compact_threshold_percent,
                compact_target_percent: config.compact_target_percent,
            },
            web_search: WebSearchConfig {
                provider: config.web_search_provider,
                openai_api_key: config.legacy_web_search_credentials.openai.as_deref(),
                exa_api_key: config.legacy_web_search_credentials.exa.as_deref(),
                brave_api_key: config.legacy_web_search_credentials.brave.as_deref(),
            },
            behavior: BehaviorConfig {
                check_for_updates: config.check_for_updates,
                enable_subagents: config.enable_subagents,
                permission_mode: config.permission_mode,
                credential_store: config.credential_store.map(CredentialStoreBackend::as_str),
                rtk: config.rtk,
                inline_shell: &config.inline_shell,
            },
            keybindings: &config.keybindings,
            prompt_templates: &config.prompt_templates,
            providers: PersistedProviderConfigs::from(&config.providers),
        }
    }
}

fn persisted_model_reference<'a>(alias: Option<&str>, model: &'a str) -> Cow<'a, str> {
    match alias {
        Some(alias) => Cow::Owned(format!("@{alias}")),
        None => Cow::Borrowed(model),
    }
}
