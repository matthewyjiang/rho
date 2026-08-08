use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, fs, path::PathBuf, str::FromStr};

use {
    crate::compaction::CompactionConfig,
    crate::credential_store::AppCredentialStore,
    crate::keybindings::Keybindings,
    crate::model_aliases::ModelAliases,
    crate::paths,
    crate::permission::PermissionMode,
    crate::tools::mcp::config::McpConfig,
    rho_providers::credentials::{
        load_web_search_api_key, save_web_search_api_key, CredentialStore, CredentialStoreBackend,
        WebSearchCredential,
    },
    rho_providers::model::catalog,
    rho_providers::model::favorites::{favorite_model_values, normalized_favorite_models},
    rho_providers::provider,
    rho_providers::reasoning::ReasoningLevel,
};

#[path = "provider_config.rs"]
mod provider_config;

#[path = "config_format.rs"]
mod format;
use format::write_config;
pub use format::InternalAgentModelConfig;
#[cfg(test)]
pub use format::{EffectiveModelConfig, EffectiveModelSource};

#[path = "config_load.rs"]
mod load;
pub(crate) use load::ConfigWarning;

pub(crate) use provider_config::ProviderConfigs;

/// Keep in lockstep with [`rho_tools::DEFAULT_MAX_OUTPUT_BYTES`].
pub(crate) const DEFAULT_MAX_OUTPUT_BYTES: usize = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;

/// Persisted application configuration owned by `rho-coding-agent`.
///
/// This type is not part of the SDK contract. Convert it through
/// `app::sdk_config::SdkBootstrapOptions`, then acquire credentials separately
/// through the application credential adapter. Provider credentials are never
/// stored in these fields; legacy web-search values are migrated to the OS
/// credential store and redact their `Debug` representation.
#[derive(Clone, Debug)]
pub struct Config {
    pub provider: String,
    pub model: String,
    /// User-defined short names for concrete models; see `ModelAliases`.
    pub model_aliases: ModelAliases,
    /// Alias the current `provider`/`model` was resolved from, if any.
    /// Consult it through `current_model_alias`, which drops it once the
    /// selection no longer matches the alias table.
    pub model_alias: Option<String>,
    pub max_output_bytes: usize,
    pub max_tool_output_lines: usize,
    pub auth: String,
    pub reasoning: ReasoningLevel,
    /// Use the low-latency priority tier for supported Codex models.
    pub fast_mode: bool,
    pub show_reasoning_output: bool,
    /// Hide tool cards, reasoning, and activity chrome so only message text remains.
    pub zen_mode: bool,
    /// Interactive TUI color theme id (`terminal`, built-in, or custom file stem).
    pub theme: String,
    pub auto_compact: bool,
    pub compact_threshold_percent: u8,
    pub compact_target_percent: u8,
    /// Optional model selections for reserved internal agents, keyed by stable agent ID.
    pub internal_agents: BTreeMap<String, InternalAgentModelConfig>,
    pub favorite_models: Vec<String>,
    /// Use the chat provider's hosted web search when the transport supports it.
    pub web_search_hosted: bool,
    /// Client-side backup backend used when hosted search is off or unsupported.
    pub web_search_provider: SearchProvider,
    pub check_for_updates: bool,
    pub enable_subagents: bool,
    /// Offer the `advisor` tool, which reviews the session with the model
    /// configured for the `advisor` internal agent.
    pub advisor_mode: bool,
    /// Enables native-tool workspace checkpoints and the experimental `/rewind` command.
    pub experimental_workspace_rewind: bool,
    pub permission_mode: PermissionMode,
    /// Explicit credential backend. `None` means unset; runtime defaults to OS.
    pub credential_store: Option<CredentialStoreBackend>,
    pub(crate) legacy_web_search_credentials: LegacyWebSearchCredentials,
    pub rtk: bool,
    pub inline_shell: String,
    pub keybindings: Keybindings,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
    pub(crate) providers: ProviderConfigs,
    pub(crate) mcp: McpConfig,
}

pub(crate) fn default_inline_shell() -> String {
    if cfg!(windows) { "powershell" } else { "bash" }.into()
}

pub(super) fn inferred_provider_auth(
    provider: &str,
    current_provider: &str,
    current_auth: &str,
) -> String {
    if provider == current_provider {
        return current_auth.into();
    }
    provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.default_auth().id.into())
        .unwrap_or_else(|| current_auth.into())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            model_aliases: ModelAliases::default(),
            model_alias: None,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_tool_output_lines: 10,
            auth: "api-key".into(),
            reasoning: ReasoningLevel::Medium,
            fast_mode: false,
            show_reasoning_output: true,
            zen_mode: false,
            theme: "terminal".into(),
            auto_compact: false,
            compact_threshold_percent: 85,
            compact_target_percent: 50,
            internal_agents: BTreeMap::new(),
            favorite_models: Vec::new(),
            web_search_hosted: true,
            web_search_provider: SearchProvider::Auto,
            check_for_updates: true,
            enable_subagents: true,
            advisor_mode: false,
            experimental_workspace_rewind: false,
            permission_mode: PermissionMode::Auto,
            credential_store: None,
            legacy_web_search_credentials: LegacyWebSearchCredentials::default(),
            rtk: true,
            inline_shell: default_inline_shell(),
            keybindings: Keybindings::default(),
            prompt_templates: Default::default(),
            providers: ProviderConfigs::default(),
            mcp: McpConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchProvider {
    #[default]
    Auto,
    OpenAi,
    Exa,
    Brave,
    Parallel,
    Tavily,
    Perplexity,
    Gemini,
    Disabled,
}

impl SearchProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::OpenAi => "openai",
            Self::Exa => "exa",
            Self::Brave => "brave",
            Self::Parallel => "parallel",
            Self::Tavily => "tavily",
            Self::Perplexity => "perplexity",
            Self::Gemini => "gemini",
            Self::Disabled => "disabled",
        }
    }

    /// Parse a configured web-search provider.
    ///
    /// Returns the resolved provider and whether the input was normalized to a
    /// different value (unsupported names become `auto`).
    pub fn parse_config_value(value: &str) -> (Self, bool) {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => (Self::Auto, false),
            "openai" => (Self::OpenAi, false),
            "exa" => (Self::Exa, false),
            "brave" => (Self::Brave, false),
            "disabled" => (Self::Disabled, false),
            _ => (Self::Auto, true),
        }
    }

    pub const fn next_configurable(self) -> Self {
        match self {
            Self::Auto => Self::OpenAi,
            Self::OpenAi => Self::Exa,
            Self::Exa => Self::Brave,
            Self::Brave => Self::Disabled,
            Self::Disabled | Self::Parallel | Self::Tavily | Self::Perplexity | Self::Gemini => {
                Self::Auto
            }
        }
    }
}

impl fmt::Display for SearchProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SearchProvider {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "openai" => Ok(Self::OpenAi),
            "exa" => Ok(Self::Exa),
            "brave" => Ok(Self::Brave),
            "parallel" => Ok(Self::Parallel),
            "tavily" => Ok(Self::Tavily),
            "perplexity" => Ok(Self::Perplexity),
            "gemini" => Ok(Self::Gemini),
            "disabled" => Ok(Self::Disabled),
            other => Err(format!("unknown search provider: {other}")),
        }
    }
}

impl Serialize for SearchProvider {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SearchProvider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct LegacyWebSearchCredentials {
    #[serde(
        default,
        rename = "web_search_openai_api_key",
        skip_serializing_if = "Option::is_none"
    )]
    openai: Option<String>,
    #[serde(
        default,
        rename = "web_search_exa_api_key",
        skip_serializing_if = "Option::is_none"
    )]
    exa: Option<String>,
    #[serde(
        default,
        rename = "web_search_brave_api_key",
        skip_serializing_if = "Option::is_none"
    )]
    brave: Option<String>,
}

impl fmt::Debug for LegacyWebSearchCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyWebSearchCredentials")
            .field("openai", &self.openai.as_ref().map(|_| "[REDACTED]"))
            .field("exa", &self.exa.as_ref().map(|_| "[REDACTED]"))
            .field("brave", &self.brave.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl LegacyWebSearchCredentials {
    fn get(&self, credential: WebSearchCredential) -> Option<&str> {
        match credential {
            WebSearchCredential::OpenAi => self.openai.as_deref(),
            WebSearchCredential::Exa => self.exa.as_deref(),
            WebSearchCredential::Brave => self.brave.as_deref(),
        }
    }

    fn clear(&mut self, credential: WebSearchCredential) {
        match credential {
            WebSearchCredential::OpenAi => self.openai = None,
            WebSearchCredential::Exa => self.exa = None,
            WebSearchCredential::Brave => self.brave = None,
        }
    }
}

impl Config {
    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(paths::rho_dir()?.join("config.toml"))
    }

    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        // Settings only. Open/migrate the credential store once at process
        // startup via credential_store::initialize_from_config.
        Self::load_settings_only(path)
    }

    /// Load config settings without opening the credential store.
    pub(crate) fn load_settings_only(path: PathBuf) -> anyhow::Result<Self> {
        if !path.exists() {
            let default = Config::default();
            default.write_settings(path.clone())?;
            return Ok(default);
        }
        Self::parse_file(path)
    }

    #[cfg(test)]
    pub(crate) fn load_with_store(
        path: PathBuf,
        store: &dyn CredentialStore,
    ) -> anyhow::Result<Self> {
        let mut cfg = Self::load_settings_only(path.clone())?;
        if matches!(cfg.migrate_legacy_web_search_credentials(store), Ok(true)) {
            let _cleanup_result = write_config(&path, &cfg);
        }
        Ok(cfg)
    }

    fn parse_file(path: PathBuf) -> anyhow::Result<Self> {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let (cfg, warnings) = Self::parse_settings_with_warnings(&text)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        load::emit_warnings(&path.display().to_string(), &warnings);
        Ok(cfg)
    }

    pub(crate) fn parse_settings(text: &str) -> anyhow::Result<Self> {
        Ok(Self::parse_settings_with_warnings(text)?.0)
    }

    pub(crate) fn parse_settings_with_warnings(
        text: &str,
    ) -> anyhow::Result<(Self, Vec<ConfigWarning>)> {
        load::parse_settings(text)
    }

    pub fn save(&self, path: Option<PathBuf>) -> anyhow::Result<()> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        self.save_with_store(path, &AppCredentialStore)
    }

    /// Write config without opening or migrating credentials.
    pub(crate) fn write_settings(&self, path: PathBuf) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut config = self.clone();
        config.normalize_compaction_percentages();
        config.favorite_models =
            favorite_model_values(&normalized_favorite_models(&config.favorite_models));
        write_config(&path, &config)
    }

    pub(crate) fn save_with_store(
        &self,
        path: PathBuf,
        store: &dyn CredentialStore,
    ) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut config = self.clone();
        config.normalize_compaction_percentages();
        config.favorite_models =
            favorite_model_values(&normalized_favorite_models(&config.favorite_models));
        let _migration_result = config.migrate_legacy_web_search_credentials(store);
        write_config(&path, &config)?;
        Ok(())
    }

    fn validate_model_aliases(&self) -> anyhow::Result<()> {
        let implemented_providers = catalog::implemented_providers();
        for (name, target) in self.model_aliases.iter() {
            let Some(provider) = target.provider.as_deref() else {
                continue;
            };
            if !implemented_providers.contains(&provider) {
                anyhow::bail!("model alias '{name}' targets unknown provider '{provider}'");
            }
        }
        Ok(())
    }

    /// Resolve the configured session model reference to its concrete target.
    ///
    /// Runs once at load time, before any model-specific behavior, so every
    /// downstream consumer sees only concrete model ids.
    fn resolve_model_alias(&mut self) -> anyhow::Result<()> {
        let resolved = self
            .model_aliases
            .resolve(&self.model)
            .map_err(|error| anyhow::anyhow!("session model: {error}"))?;
        self.model_alias = resolved.alias;
        if let Some(provider) = resolved
            .provider
            .as_deref()
            .filter(|provider| *provider != self.provider)
        {
            if let Some(descriptor) = provider::provider_descriptor(provider) {
                self.auth = descriptor.default_auth().id.into();
            }
            self.provider = provider.to_string();
        }
        self.model = resolved.model;
        Ok(())
    }

    fn resolve_internal_agent_model_aliases(&mut self) -> anyhow::Result<()> {
        for (id, selection) in &mut self.internal_agents {
            let resolved = self
                .model_aliases
                .resolve(&selection.model)
                .map_err(|error| anyhow::anyhow!("internal agent '{id}' model: {error}"))?;
            selection.model_alias = resolved.alias;
            if let Some(provider) = resolved.provider {
                if selection.provider != provider {
                    if let Some(descriptor) = provider::provider_descriptor(&provider) {
                        selection.auth = descriptor.default_auth().id.into();
                    }
                    selection.provider = provider;
                }
            }
            selection.model = resolved.model;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn effective_internal_agent_model(&self, id: &str) -> EffectiveModelConfig {
        match self.internal_agents.get(id) {
            Some(selection) => EffectiveModelConfig {
                provider: selection.provider.clone(),
                model: selection.model.clone(),
                auth: selection.auth.clone(),
                source: EffectiveModelSource::Override,
            },
            None => EffectiveModelConfig {
                provider: self.provider.clone(),
                model: self.model.clone(),
                auth: self.auth.clone(),
                source: EffectiveModelSource::Conversation,
            },
        }
    }

    #[cfg(test)]
    pub fn set_internal_agent_model(
        &mut self,
        id: impl Into<String>,
        provider: String,
        model: String,
        auth: String,
    ) {
        self.set_internal_agent_model_config(
            id,
            InternalAgentModelConfig::new(provider, model, auth),
        );
    }

    pub fn set_internal_agent_model_config(
        &mut self,
        id: impl Into<String>,
        selection: InternalAgentModelConfig,
    ) {
        self.internal_agents.insert(id.into(), selection);
    }

    pub fn clear_internal_agent_model(&mut self, id: &str) {
        self.internal_agents.remove(id);
    }

    /// The model explicitly configured for an internal agent, with no
    /// conversation-model fallback. Agents that need their own model
    /// (see `internal_agent_requires_model`) read this instead of
    /// `effective_internal_agent_model`.
    pub fn internal_agent_model(&self, id: &str) -> Option<&InternalAgentModelConfig> {
        self.internal_agents.get(id)
    }

    #[cfg(test)]
    pub fn current_internal_agent_model_alias(&self, id: &str) -> Option<&str> {
        self.internal_agents
            .get(id)?
            .current_alias(&self.model_aliases)
    }

    /// The alias behind the current model selection, provided the alias table
    /// still maps it there; stale aliases silently drop out.
    pub fn current_model_alias(&self) -> Option<&str> {
        let name = self.model_alias.as_deref()?;
        let target = self.model_aliases.get(name)?;
        (target.model == self.model
            && target.provider.as_deref().unwrap_or(&self.provider) == self.provider)
            .then_some(name)
    }

    pub fn set_compact_threshold_percent(&mut self, value: u8) {
        self.compact_threshold_percent = clamp_percent(value);
        self.normalize_compaction_percentages();
    }

    pub fn set_compact_target_percent(&mut self, value: u8) {
        self.compact_target_percent = clamp_percent(value);
        self.normalize_compaction_percentages();
    }

    pub(crate) fn legacy_web_search_api_key(
        &self,
        credential: WebSearchCredential,
    ) -> Option<&str> {
        self.legacy_web_search_credentials.get(credential)
    }

    pub(crate) fn migrate_legacy_web_search_credentials(
        &mut self,
        store: &dyn CredentialStore,
    ) -> rho_providers::credentials::CredentialResult<bool> {
        let mut changed = false;
        for credential in WebSearchCredential::ALL {
            let Some(secret) = self
                .legacy_web_search_credentials
                .get(credential)
                .map(str::to_string)
            else {
                continue;
            };
            if load_web_search_api_key(store, credential)?.is_none() {
                save_web_search_api_key(store, credential, &secret)?;
            }
            self.legacy_web_search_credentials.clear(credential);
            changed = true;
        }
        Ok(changed)
    }

    fn normalize_compaction_percentages(&mut self) {
        self.compact_threshold_percent = clamp_percent(self.compact_threshold_percent);
        self.compact_target_percent = normalized_compact_target_percent(
            self.compact_threshold_percent,
            self.compact_target_percent,
        );
    }
}

impl From<&Config> for CompactionConfig {
    fn from(config: &Config) -> Self {
        Self {
            auto_compact: config.auto_compact,
            threshold_percent: config.compact_threshold_percent,
            target_percent: config.compact_target_percent,
        }
    }
}

fn clamp_percent(value: u8) -> u8 {
    value.clamp(1, 100)
}

fn normalized_compact_target_percent(threshold_percent: u8, target_percent: u8) -> u8 {
    let threshold_percent = clamp_percent(threshold_percent);
    let target_percent = clamp_percent(target_percent);
    if threshold_percent == 1 {
        1
    } else {
        target_percent.min(threshold_percent - 1)
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
