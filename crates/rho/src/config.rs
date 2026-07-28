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
    /// Use the low-latency priority tier for supported Codex models.
    pub fast_mode: bool,
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
        let file = toml::from_str::<PartialConfig>(&text)?.normalize_legacy()?;
        if let Some(v) = file.prompt_templates {
            crate::prompt_templates::validate(&v)?;
            cfg.prompt_templates = v;
        }
        if let Some(v) = file.provider {
            cfg.provider = v;
        }
        if let Some(v) = file.auth {
            cfg.auth = v;
        }
        if let Some(v) = file.reasoning {
            cfg.reasoning = v;
        }
        if let Some(v) = file.fast_mode {
            cfg.fast_mode = v;
        }
        if let Some(v) = file.favorite_models {
            cfg.favorite_models = favorite_model_values(&normalized_favorite_models(&v));
        }
        match file.model {
            Some(ModelSetting::Name(model)) => cfg.model = model,
            Some(ModelSetting::Group(group)) => {
                if let Some(provider) = group.provider {
                    cfg.provider = provider;
                }
                if let Some(model) = group.model {
                    cfg.model = model;
                }
                if let Some(auth) = group.auth {
                    cfg.auth = auth;
                }
                if let Some(reasoning) = group.reasoning {
                    cfg.reasoning = reasoning;
                }
                if let Some(fast_mode) = group.fast_mode {
                    cfg.fast_mode = fast_mode;
                }
                if let Some(models) = group.favorite_models {
                    cfg.favorite_models =
                        favorite_model_values(&normalized_favorite_models(&models));
                }
                if let Some(aliases) = group.aliases {
                    cfg.model_aliases = aliases;
                }
            }
            None => {}
        }
        cfg.validate_model_aliases()?;
        cfg.resolve_model_alias()?;
        if let Some(group) = file.display {
            if let Some(value) = group.show_reasoning_output {
                cfg.show_reasoning_output = value;
            }
            if let Some(value) = group.max_tool_output_lines {
                cfg.max_tool_output_lines = value.max(1);
            }
        }
        if let Some(group) = file.output {
            if let Some(value) = group.max_output_bytes {
                cfg.max_output_bytes = value;
            }
        }
        if let Some(group) = file.compaction {
            if let Some(value) = group.auto_compact {
                cfg.auto_compact = value;
            }
            if let Some(value) = group.compact_threshold_percent {
                cfg.set_compact_threshold_percent(value);
            }
            if let Some(value) = group.compact_target_percent {
                cfg.set_compact_target_percent(value);
            }
        }
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
        if let Some(group) = file.title {
            let provider = group.provider.unwrap_or_else(|| cfg.provider.clone());
            let auth = group
                .auth
                .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
            cfg.internal_agents
                .entry("session-title".into())
                .or_insert(InternalAgentModelConfig {
                    provider,
                    model: group.model.unwrap_or_else(|| cfg.model.clone()),
                    auth,
                    model_alias: None,
                });
        }
        cfg.resolve_internal_agent_model_aliases()?;
        cfg.normalize_provider_profiles()?;
        if let Some(group) = file.web_search {
            if let Some(provider) = group.provider {
                cfg.web_search_provider = SearchProvider::from_config_value(&provider);
            }
            cfg.legacy_web_search_credentials = LegacyWebSearchCredentials {
                openai: group.openai_api_key.and_then(non_empty_secret),
                exa: group.exa_api_key.and_then(non_empty_secret),
                brave: group.brave_api_key.and_then(non_empty_secret),
            };
        }
        if let Some(providers) = file.providers {
            cfg.providers.apply(providers)?;
        }
        if let Some(group) = file.behavior {
            if let Some(value) = group.check_for_updates {
                cfg.check_for_updates = value;
            }
            if let Some(value) = group.enable_subagents {
                cfg.enable_subagents = value;
            }
            if let Some(value) = group.permission_mode {
                cfg.permission_mode = value;
            }
            if let Some(value) = group.credential_store.as_deref() {
                cfg.credential_store = Some(
                    CredentialStoreBackend::parse(value)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                );
            }
            if let Some(value) = group.rtk {
                cfg.rtk = value;
            }
            if let Some(value) = group.inline_shell.filter(|value| !value.trim().is_empty()) {
                cfg.inline_shell = value;
            }
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
    fast_mode: Option<bool>,
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

impl PartialConfig {
    /// Fold every legacy top-level key into its modern group.
    ///
    /// Group values win when both a flat key and a group field are present,
    /// matching the previous parse order where groups were applied second.
    fn normalize_legacy(mut self) -> anyhow::Result<Self> {
        if self.reasoning.is_none() {
            if let Some(effort) = self.reasoning_effort.take() {
                self.reasoning = Some(effort.parse()?);
            }
        } else {
            self.reasoning_effort = None;
        }

        let show_reasoning_output = self.show_reasoning_output.take();
        let max_tool_output_lines = self.max_tool_output_lines.take();
        if show_reasoning_output.is_some()
            || max_tool_output_lines.is_some()
            || self.display.is_some()
        {
            let group = self.display.take().unwrap_or(PartialDisplayConfig {
                show_reasoning_output: None,
                max_tool_output_lines: None,
            });
            self.display = Some(PartialDisplayConfig {
                show_reasoning_output: group.show_reasoning_output.or(show_reasoning_output),
                max_tool_output_lines: group.max_tool_output_lines.or(max_tool_output_lines),
            });
        }

        let max_output_bytes = self.max_output_bytes.take();
        if max_output_bytes.is_some() || self.output.is_some() {
            let group = self.output.take().unwrap_or(PartialOutputConfig {
                max_output_bytes: None,
            });
            self.output = Some(PartialOutputConfig {
                max_output_bytes: group.max_output_bytes.or(max_output_bytes),
            });
        }

        let auto_compact = self.auto_compact.take();
        let compact_threshold_percent = self.compact_threshold_percent.take();
        let compact_target_percent = self.compact_target_percent.take();
        if auto_compact.is_some()
            || compact_threshold_percent.is_some()
            || compact_target_percent.is_some()
            || self.compaction.is_some()
        {
            let group = self.compaction.take().unwrap_or(PartialCompactionConfig {
                auto_compact: None,
                compact_threshold_percent: None,
                compact_target_percent: None,
            });
            self.compaction = Some(PartialCompactionConfig {
                auto_compact: group.auto_compact.or(auto_compact),
                compact_threshold_percent: group
                    .compact_threshold_percent
                    .or(compact_threshold_percent),
                compact_target_percent: group.compact_target_percent.or(compact_target_percent),
            });
        }

        let check_for_updates = self.check_for_updates.take();
        let enable_subagents = self.enable_subagents.take();
        let permission_mode = self.permission_mode.take();
        let rtk = self.rtk.take();
        let inline_shell = self.inline_shell.take();
        if check_for_updates.is_some()
            || enable_subagents.is_some()
            || permission_mode.is_some()
            || rtk.is_some()
            || inline_shell.is_some()
            || self.behavior.is_some()
        {
            let group = self.behavior.take().unwrap_or(PartialBehaviorConfig {
                check_for_updates: None,
                enable_subagents: None,
                permission_mode: None,
                credential_store: None,
                rtk: None,
                inline_shell: None,
            });
            self.behavior = Some(PartialBehaviorConfig {
                check_for_updates: group.check_for_updates.or(check_for_updates),
                enable_subagents: group.enable_subagents.or(enable_subagents),
                permission_mode: group.permission_mode.or(permission_mode),
                credential_store: group.credential_store,
                rtk: group.rtk.or(rtk),
                inline_shell: group.inline_shell.or(inline_shell),
            });
        }

        let web_search_provider = self.web_search_provider.take();
        let openai_api_key = self.web_search_openai_api_key.take();
        let exa_api_key = self.web_search_exa_api_key.take();
        let brave_api_key = self.web_search_brave_api_key.take();
        if web_search_provider.is_some()
            || openai_api_key.is_some()
            || exa_api_key.is_some()
            || brave_api_key.is_some()
            || self.web_search.is_some()
        {
            let group = self.web_search.take().unwrap_or(PartialWebSearchConfig {
                provider: None,
                openai_api_key: None,
                exa_api_key: None,
                brave_api_key: None,
            });
            self.web_search = Some(PartialWebSearchConfig {
                provider: group.provider.or(web_search_provider),
                openai_api_key: group.openai_api_key.or(openai_api_key),
                exa_api_key: group.exa_api_key.or(exa_api_key),
                brave_api_key: group.brave_api_key.or(brave_api_key),
            });
        }

        let title_provider = self.title_provider.take();
        let title_model = self.title_model.take();
        let title_auth = self.title_auth.take();
        match self.title.take() {
            Some(group)
                if group.provider.is_none() && group.model.is_none() && group.auth.is_none() =>
            {
                // Empty `[title]` is a no-op. Legacy top-level title_* keys still apply.
                if title_provider.is_some() || title_model.is_some() || title_auth.is_some() {
                    self.title = Some(PartialTitleConfig {
                        provider: title_provider,
                        model: title_model,
                        auth: title_auth,
                    });
                }
            }
            Some(group) => {
                self.title = Some(PartialTitleConfig {
                    provider: group.provider.or(title_provider),
                    model: group.model.or(title_model),
                    auth: group.auth.or(title_auth),
                });
            }
            None if title_provider.is_some() || title_model.is_some() || title_auth.is_some() => {
                self.title = Some(PartialTitleConfig {
                    provider: title_provider,
                    model: title_model,
                    auth: title_auth,
                });
            }
            None => {}
        }

        self.model = match self.model.take() {
            Some(ModelSetting::Group(mut group)) => {
                group.provider = group.provider.or(self.provider.take());
                group.auth = group.auth.or(self.auth.take());
                group.reasoning = group.reasoning.or(self.reasoning.take());
                group.fast_mode = group.fast_mode.or(self.fast_mode.take());
                group.favorite_models = group.favorite_models.or(self.favorite_models.take());
                Some(ModelSetting::Group(group))
            }
            other => other,
        };

        Ok(self)
    }
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
    fast_mode: Option<bool>,
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
