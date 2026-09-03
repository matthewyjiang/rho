use std::{borrow::Cow, collections::BTreeMap, path::Path};

use serde::Serialize;

use {
    crate::keybindings::Keybindings, crate::model_aliases::ModelAliases,
    crate::permission::PermissionMode, rho_providers::credentials::CredentialStoreBackend,
    rho_providers::reasoning::ReasoningLevel,
};

use super::{
    provider_config::PersistedProviderConfigs, Config, EditTool, InternalAgentModelConfig,
    InternalAgentTarget, SearchProvider,
};

pub(super) fn write_config(path: &Path, config: &Config) -> anyhow::Result<()> {
    let serialized = toml::to_string_pretty(&GroupedConfig::from(config))?;
    crate::config_writer::write_atomically(path, &serialized)
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

#[derive(Serialize)]
struct GroupedConfig<'a> {
    model: ModelConfig<'a>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    internal_agents: BTreeMap<&'a str, PersistedInternalAgentModelConfig<'a>>,
    display: DisplayConfig,
    output: OutputConfig,
    compaction: CompactionSection,
    web_search: WebSearchConfig<'a>,
    #[serde(skip_serializing_if = "xai_config_is_default")]
    xai: XaiConfig,
    behavior: BehaviorConfig<'a>,
    keybindings: &'a Keybindings,
    prompt_templates: &'a crate::prompt_templates::PromptTemplates,
    #[serde(skip_serializing_if = "crate::tools::mcp::config::McpConfig::is_empty")]
    mcp: &'a crate::tools::mcp::config::McpConfig,
    #[serde(skip_serializing_if = "PersistedProviderConfigs::is_empty")]
    providers: PersistedProviderConfigs<'a>,
}

#[derive(Serialize)]
struct ModelConfig<'a> {
    provider: &'a str,
    model: Cow<'a, str>,
    auth: &'a str,
    reasoning: ReasoningLevel,
    fast_mode: bool,
    favorite_models: &'a [String],
    #[serde(skip_serializing_if = "ModelAliases::is_empty")]
    aliases: &'a ModelAliases,
}

#[derive(Serialize)]
struct DisplayConfig {
    show_reasoning_output: bool,
    zen_mode: bool,
    theme: String,
    max_tool_output_lines: usize,
    prompt_history_limit: usize,
    cache_miss_notices: bool,
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

/// On-disk form of one `[internal_agents.<id>]` entry.
///
/// `runtime` is omitted for Rho selections, so files written before the Claude
/// Code runtime existed round-trip unchanged. A `claude-cli` entry carries only
/// the pass-through model: it has no Rho provider and no Rho auth.
#[derive(Serialize)]
struct PersistedInternalAgentModelConfig<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningLevel>,
}

impl<'a> PersistedInternalAgentModelConfig<'a> {
    fn new(selection: &'a InternalAgentModelConfig, aliases: &'a ModelAliases) -> Self {
        let (runtime, provider, model, auth) = match &selection.target {
            InternalAgentTarget::Rho(rho) => (
                None,
                Some(rho.provider.as_str()),
                Some(persisted_model_reference(
                    selection.current_alias(aliases),
                    &rho.model,
                )),
                Some(rho.auth.as_str()),
            ),
            InternalAgentTarget::ClaudeCli { model } => (
                Some(CLAUDE_CLI_RUNTIME_KEY),
                None,
                model.as_deref().map(Cow::Borrowed),
                None,
            ),
        };
        Self {
            runtime,
            provider,
            model,
            auth,
            reasoning: selection.reasoning,
        }
    }
}

/// `runtime` value that selects the Claude Code CLI for an internal agent.
/// Matches the agent frontmatter vocabulary (`runtime: claude-cli`).
pub const CLAUDE_CLI_RUNTIME_KEY: &str = "claude-cli";
pub const RHO_RUNTIME_KEY: &str = "rho";
pub const CURSOR_RUNTIME_KEY: &str = "cursor";

#[derive(Serialize)]
struct XaiConfig {
    image_generation: bool,
}

fn xai_config_is_default(config: &XaiConfig) -> bool {
    config.image_generation
}

#[derive(Serialize)]
struct WebSearchConfig<'a> {
    hosted: bool,
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
    agent_concurrency: usize,
    advisor_mode: bool,
    experimental_workspace_rewind: bool,
    edit_tool: EditTool,
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
                fast_mode: config.fast_mode,
                favorite_models: &config.favorite_models,
                aliases: &config.model_aliases,
            },
            internal_agents: config
                .internal_agents
                .iter()
                .map(|(id, selection)| {
                    (
                        id.as_str(),
                        PersistedInternalAgentModelConfig::new(selection, &config.model_aliases),
                    )
                })
                .collect(),
            display: DisplayConfig {
                show_reasoning_output: config.show_reasoning_output,
                zen_mode: config.zen_mode,
                theme: config.theme.clone(),
                max_tool_output_lines: config.max_tool_output_lines,
                prompt_history_limit: config.prompt_history_limit,
                cache_miss_notices: config.cache_miss_notices,
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
                hosted: config.web_search_hosted,
                provider: config.web_search_provider,
                openai_api_key: config.legacy_web_search_credentials.openai.as_deref(),
                exa_api_key: config.legacy_web_search_credentials.exa.as_deref(),
                brave_api_key: config.legacy_web_search_credentials.brave.as_deref(),
            },
            xai: XaiConfig {
                image_generation: config.xai_image_generation,
            },
            behavior: BehaviorConfig {
                check_for_updates: config.check_for_updates,
                enable_subagents: config.enable_subagents,
                agent_concurrency: config.agent_concurrency,
                advisor_mode: config.advisor_mode,
                experimental_workspace_rewind: config.experimental_workspace_rewind,
                edit_tool: config.edit_tool,
                permission_mode: config.permission_mode,
                credential_store: config.credential_store.map(CredentialStoreBackend::as_str),
                rtk: config.rtk,
                inline_shell: &config.inline_shell,
            },
            keybindings: &config.keybindings,
            prompt_templates: &config.prompt_templates,
            mcp: &config.mcp,
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
