use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, fs, path::PathBuf, str::FromStr};

use {
    crate::compaction::CompactionConfig,
    crate::credential_store::AppCredentialStore,
    crate::keybindings::Keybindings,
    crate::model_aliases::ModelAliases,
    crate::paths,
    crate::permission::PermissionMode,
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

use provider_config::PartialProviderConfigs;
pub(crate) use provider_config::ProviderConfigs;
#[cfg(test)]
use provider_config::DEFAULT_OLLAMA_BASE_URL;

pub(crate) const DEFAULT_MAX_OUTPUT_BYTES: usize = 12_000;

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
    pub show_reasoning_output: bool,
    pub auto_compact: bool,
    pub compact_threshold_percent: u8,
    pub compact_target_percent: u8,
    /// Optional model selections for reserved internal agents, keyed by stable agent ID.
    pub internal_agents: BTreeMap<String, InternalAgentModelConfig>,
    pub favorite_models: Vec<String>,
    pub web_search_provider: SearchProvider,
    pub check_for_updates: bool,
    pub enable_subagents: bool,
    pub permission_mode: PermissionMode,
    /// Explicit credential backend. `None` means unset; runtime defaults to OS.
    pub credential_store: Option<CredentialStoreBackend>,
    pub(crate) legacy_web_search_credentials: LegacyWebSearchCredentials,
    pub rtk: bool,
    pub inline_shell: String,
    pub keybindings: Keybindings,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
    pub(crate) providers: ProviderConfigs,
}

pub(crate) fn default_inline_shell() -> String {
    if cfg!(windows) { "powershell" } else { "bash" }.into()
}

fn inferred_provider_auth(provider: &str, current_provider: &str, current_auth: &str) -> String {
    if provider == current_provider {
        return current_auth.into();
    }
    provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.auth.into())
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
            show_reasoning_output: true,
            auto_compact: false,
            compact_threshold_percent: 85,
            compact_target_percent: 50,
            internal_agents: BTreeMap::new(),
            favorite_models: Vec::new(),
            web_search_provider: SearchProvider::Auto,
            check_for_updates: true,
            enable_subagents: true,
            permission_mode: PermissionMode::Auto,
            credential_store: None,
            legacy_web_search_credentials: LegacyWebSearchCredentials::default(),
            rtk: true,
            inline_shell: default_inline_shell(),
            keybindings: Keybindings::default(),
            prompt_templates: Default::default(),
            providers: ProviderConfigs::default(),
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

    pub fn from_config_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "openai" => Self::OpenAi,
            "exa" => Self::Exa,
            "brave" => Self::Brave,
            "disabled" => Self::Disabled,
            _ => Self::Auto,
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
        let mut cfg = Config::default();
        let text = fs::read_to_string(&path)?;
        let file: PartialConfig = toml::from_str(&text)?;
        let legacy_title_provider = file.title_provider.clone();
        let legacy_title_model = file.title_model.clone();
        let legacy_title_auth = file.title_auth.clone();
        if let Some(v) = file.prompt_templates {
            crate::prompt_templates::validate(&v)?;
            cfg.prompt_templates = v;
        }
        if let Some(v) = file.provider {
            cfg.provider = v;
        }
        if let Some(ModelSetting::Name(v)) = file.model.as_ref() {
            cfg.model = v.clone();
        }
        if let Some(v) = file.max_output_bytes {
            cfg.max_output_bytes = v;
        }
        if let Some(v) = file.max_tool_output_lines {
            cfg.max_tool_output_lines = v.max(1);
        }
        if let Some(v) = file.auth {
            cfg.auth = v;
        }
        if let Some(v) = file.reasoning {
            cfg.reasoning = v;
        } else if let Some(v) = file.reasoning_effort {
            cfg.reasoning = v.parse()?;
        }
        if let Some(v) = file.show_reasoning_output {
            cfg.show_reasoning_output = v;
        }
        if let Some(v) = file.auto_compact {
            cfg.auto_compact = v;
        }
        if let Some(v) = file.compact_threshold_percent {
            cfg.set_compact_threshold_percent(v);
        }
        if let Some(v) = file.compact_target_percent {
            cfg.set_compact_target_percent(v);
        }
        if let Some(v) = file.favorite_models {
            cfg.favorite_models = favorite_model_values(&normalized_favorite_models(&v));
        }
        if let Some(v) = file.web_search_provider {
            cfg.web_search_provider = SearchProvider::from_config_value(&v);
        }
        if let Some(v) = file.check_for_updates {
            cfg.check_for_updates = v;
        }
        if let Some(v) = file.enable_subagents {
            cfg.enable_subagents = v;
        }
        if let Some(v) = file.permission_mode {
            cfg.permission_mode = v;
        }
        cfg.legacy_web_search_credentials = LegacyWebSearchCredentials {
            openai: file.web_search_openai_api_key.and_then(non_empty_secret),
            exa: file.web_search_exa_api_key.and_then(non_empty_secret),
            brave: file.web_search_brave_api_key.and_then(non_empty_secret),
        };
        if let Some(v) = file.rtk {
            cfg.rtk = v;
        }
        if let Some(v) = file.inline_shell.filter(|value| !value.trim().is_empty()) {
            cfg.inline_shell = v;
        }
        if let Some(ModelSetting::Group(group)) = file.model {
            cfg.provider = group.provider.unwrap_or(cfg.provider);
            cfg.model = group.model.unwrap_or(cfg.model);
            cfg.auth = group.auth.unwrap_or(cfg.auth);
            cfg.reasoning = group.reasoning.unwrap_or(cfg.reasoning);
            cfg.favorite_models = group
                .favorite_models
                .map(|models| favorite_model_values(&normalized_favorite_models(&models)))
                .unwrap_or(cfg.favorite_models);
            cfg.model_aliases = group.aliases.unwrap_or(cfg.model_aliases);
        }
        cfg.validate_model_aliases()?;
        cfg.resolve_model_alias()?;
        if let Some(group) = file.display {
            cfg.show_reasoning_output = group
                .show_reasoning_output
                .unwrap_or(cfg.show_reasoning_output);
            cfg.max_tool_output_lines = group
                .max_tool_output_lines
                .unwrap_or(cfg.max_tool_output_lines)
                .max(1);
        }
        if let Some(group) = file.output {
            cfg.max_output_bytes = group.max_output_bytes.unwrap_or(cfg.max_output_bytes);
        }
        if let Some(group) = file.compaction {
            cfg.auto_compact = group.auto_compact.unwrap_or(cfg.auto_compact);
            if let Some(value) = group.compact_threshold_percent {
                cfg.set_compact_threshold_percent(value);
            }
            if let Some(value) = group.compact_target_percent {
                cfg.set_compact_target_percent(value);
            }
        }
        let legacy_title = file
            .title
            .and_then(|group| {
                if group.provider.is_none() && group.model.is_none() && group.auth.is_none() {
                    return None;
                }
                let provider = group
                    .provider
                    .or_else(|| legacy_title_provider.clone())
                    .unwrap_or_else(|| cfg.provider.clone());
                let auth = group
                    .auth
                    .or_else(|| legacy_title_auth.clone())
                    .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
                Some(InternalAgentModelConfig {
                    provider,
                    model: group
                        .model
                        .or_else(|| legacy_title_model.clone())
                        .unwrap_or_else(|| cfg.model.clone()),
                    auth,
                    model_alias: None,
                })
            })
            .or_else(|| {
                (legacy_title_provider.is_some()
                    || legacy_title_model.is_some()
                    || legacy_title_auth.is_some())
                .then(|| {
                    let provider = legacy_title_provider
                        .clone()
                        .unwrap_or_else(|| cfg.provider.clone());
                    let auth = legacy_title_auth.clone().unwrap_or_else(|| {
                        inferred_provider_auth(&provider, &cfg.provider, &cfg.auth)
                    });
                    InternalAgentModelConfig {
                        provider,
                        model: legacy_title_model
                            .clone()
                            .unwrap_or_else(|| cfg.model.clone()),
                        auth,
                        model_alias: None,
                    }
                })
            });
        cfg.internal_agents = file
            .internal_agents
            .unwrap_or_default()
            .into_iter()
            .map(|(id, group)| {
                let provider = group.provider.unwrap_or_else(|| cfg.provider.clone());
                let auth = group
                    .auth
                    .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
                (
                    id,
                    InternalAgentModelConfig {
                        provider,
                        model: group.model.unwrap_or_else(|| cfg.model.clone()),
                        auth,
                        model_alias: None,
                    },
                )
            })
            .collect();
        if let Some(selection) = legacy_title {
            cfg.internal_agents
                .entry("session-title".into())
                .or_insert(selection);
        }
        cfg.resolve_internal_agent_model_aliases()?;
        cfg.normalize_provider_profiles()?;
        if let Some(group) = file.web_search {
            if let Some(provider) = group.provider {
                cfg.web_search_provider = SearchProvider::from_config_value(&provider);
            }
            if let Some(secret) = group.openai_api_key.and_then(non_empty_secret) {
                cfg.legacy_web_search_credentials.openai = Some(secret);
            }
            if let Some(secret) = group.exa_api_key.and_then(non_empty_secret) {
                cfg.legacy_web_search_credentials.exa = Some(secret);
            }
            if let Some(secret) = group.brave_api_key.and_then(non_empty_secret) {
                cfg.legacy_web_search_credentials.brave = Some(secret);
            }
        }
        if let Some(providers) = file.providers {
            cfg.providers.apply(providers)?;
        }
        if let Some(group) = file.behavior {
            cfg.check_for_updates = group.check_for_updates.unwrap_or(cfg.check_for_updates);
            cfg.enable_subagents = group.enable_subagents.unwrap_or(cfg.enable_subagents);
            cfg.permission_mode = group.permission_mode.unwrap_or(cfg.permission_mode);
            if let Some(value) = group.credential_store.as_deref() {
                cfg.credential_store = Some(
                    CredentialStoreBackend::parse(value)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                );
            }
            cfg.rtk = group.rtk.unwrap_or(cfg.rtk);
            cfg.inline_shell = group
                .inline_shell
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(cfg.inline_shell);
        }
        if let Some(keybindings) = file.keybindings {
            cfg.keybindings = keybindings;
        }
        Ok(cfg)
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
                self.auth = descriptor.auth.into();
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
                        selection.auth = descriptor.auth.into();
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

    pub fn set_internal_agent_model(
        &mut self,
        id: impl Into<String>,
        provider: String,
        model: String,
        auth: String,
    ) {
        self.internal_agents.insert(
            id.into(),
            InternalAgentModelConfig {
                provider,
                model,
                auth,
                model_alias: None,
            },
        );
    }

    pub fn clear_internal_agent_model(&mut self, id: &str) {
        self.internal_agents.remove(id);
    }

    #[cfg(test)]
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

#[derive(Deserialize)]
struct PartialConfig {
    provider: Option<String>,
    model: Option<ModelSetting>,
    max_output_bytes: Option<usize>,
    max_tool_output_lines: Option<usize>,
    auth: Option<String>,
    reasoning: Option<ReasoningLevel>,
    reasoning_effort: Option<String>,
    show_reasoning_output: Option<bool>,
    auto_compact: Option<bool>,
    compact_threshold_percent: Option<u8>,
    compact_target_percent: Option<u8>,
    title_provider: Option<String>,
    title_model: Option<String>,
    title_auth: Option<String>,
    favorite_models: Option<Vec<String>>,
    web_search_provider: Option<String>,
    check_for_updates: Option<bool>,
    enable_subagents: Option<bool>,
    #[serde(default)]
    permission_mode: Option<PermissionMode>,
    web_search_openai_api_key: Option<String>,
    web_search_exa_api_key: Option<String>,
    web_search_brave_api_key: Option<String>,
    rtk: Option<bool>,
    inline_shell: Option<String>,
    display: Option<PartialDisplayConfig>,
    output: Option<PartialOutputConfig>,
    compaction: Option<PartialCompactionConfig>,
    title: Option<PartialTitleConfig>,
    internal_agents: Option<BTreeMap<String, PartialInternalAgentModelConfig>>,
    web_search: Option<PartialWebSearchConfig>,
    behavior: Option<PartialBehaviorConfig>,
    keybindings: Option<Keybindings>,
    prompt_templates: Option<crate::prompt_templates::PromptTemplates>,
    providers: Option<PartialProviderConfigs>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelSetting {
    Name(String),
    Group(PartialModelConfig),
}

#[derive(Deserialize)]
struct PartialModelConfig {
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
    reasoning: Option<ReasoningLevel>,
    favorite_models: Option<Vec<String>>,
    aliases: Option<ModelAliases>,
}

#[derive(Deserialize)]
struct PartialDisplayConfig {
    show_reasoning_output: Option<bool>,
    max_tool_output_lines: Option<usize>,
}

#[derive(Deserialize)]
struct PartialOutputConfig {
    max_output_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct PartialCompactionConfig {
    auto_compact: Option<bool>,
    compact_threshold_percent: Option<u8>,
    compact_target_percent: Option<u8>,
}

#[derive(Deserialize)]
struct PartialInternalAgentModelConfig {
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
}

#[derive(Deserialize)]
struct PartialTitleConfig {
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
}

#[derive(Deserialize)]
struct PartialWebSearchConfig {
    provider: Option<String>,
    openai_api_key: Option<String>,
    exa_api_key: Option<String>,
    brave_api_key: Option<String>,
}

#[derive(Deserialize)]
struct PartialBehaviorConfig {
    check_for_updates: Option<bool>,
    enable_subagents: Option<bool>,
    #[serde(default)]
    permission_mode: Option<PermissionMode>,
    credential_store: Option<String>,
    rtk: Option<bool>,
    inline_shell: Option<String>,
}

fn non_empty_secret(secret: String) -> Option<String> {
    let secret = secret.trim().to_string();
    (!secret.is_empty()).then_some(secret)
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
#[path = "config_atomic_tests.rs"]
mod atomic_tests;
#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
