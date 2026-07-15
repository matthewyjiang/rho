use serde::{Deserialize, Serialize};
use std::{fmt, fs, path::PathBuf, str::FromStr};

use crate::{
    compaction::CompactionConfig,
    credentials::{
        load_web_search_api_key, save_web_search_api_key, CredentialStore, OsCredentialStore,
        WebSearchCredential,
    },
    keybindings::Keybindings,
    model::favorites::{favorite_model_values, normalized_favorite_models},
    paths,
    reasoning::ReasoningLevel,
};

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
    pub max_output_bytes: usize,
    pub max_tool_output_lines: usize,
    pub auth: String,
    pub reasoning: ReasoningLevel,
    pub show_reasoning_output: bool,
    pub auto_compact: bool,
    pub compact_threshold_percent: u8,
    pub compact_target_percent: u8,
    pub title_provider: Option<String>,
    pub title_model: Option<String>,
    pub title_auth: Option<String>,
    pub favorite_models: Vec<String>,
    pub web_search_provider: SearchProvider,
    pub check_for_updates: bool,
    pub(crate) legacy_web_search_credentials: LegacyWebSearchCredentials,
    pub rtk: bool,
    pub inline_shell: String,
    pub keybindings: Keybindings,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
}

pub(crate) fn default_inline_shell() -> String {
    if cfg!(windows) { "powershell" } else { "bash" }.into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            max_output_bytes: 12000,
            max_tool_output_lines: 10,
            auth: "api-key".into(),
            reasoning: ReasoningLevel::Medium,
            show_reasoning_output: true,
            auto_compact: false,
            compact_threshold_percent: 85,
            compact_target_percent: 50,
            title_provider: None,
            title_model: None,
            title_auth: None,
            favorite_models: Vec::new(),
            web_search_provider: SearchProvider::Auto,
            check_for_updates: true,
            legacy_web_search_credentials: LegacyWebSearchCredentials::default(),
            rtk: true,
            inline_shell: default_inline_shell(),
            keybindings: Keybindings::default(),
            prompt_templates: Default::default(),
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

fn write_config(path: &PathBuf, config: &Config) -> anyhow::Result<()> {
    let serialized = toml::to_string_pretty(&GroupedConfig::from(config))?;
    crate::config_writer::write_atomically(path, &serialized)
}

#[derive(Serialize)]
struct GroupedConfig<'a> {
    model: ModelConfig<'a>,
    display: DisplayConfig,
    output: OutputConfig,
    compaction: CompactionSection,
    title: TitleConfig<'a>,
    web_search: WebSearchConfig<'a>,
    behavior: BehaviorConfig<'a>,
    keybindings: &'a Keybindings,
    prompt_templates: &'a crate::prompt_templates::PromptTemplates,
}

#[derive(Serialize)]
struct ModelConfig<'a> {
    provider: &'a str,
    model: &'a str,
    auth: &'a str,
    reasoning: ReasoningLevel,
    favorite_models: &'a [String],
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
struct TitleConfig<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<&'a str>,
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
    rtk: bool,
    inline_shell: &'a str,
}

impl<'a> From<&'a Config> for GroupedConfig<'a> {
    fn from(config: &'a Config) -> Self {
        Self {
            model: ModelConfig {
                provider: &config.provider,
                model: &config.model,
                auth: &config.auth,
                reasoning: config.reasoning,
                favorite_models: &config.favorite_models,
            },
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
            title: TitleConfig {
                provider: config.title_provider.as_deref(),
                model: config.title_model.as_deref(),
                auth: config.title_auth.as_deref(),
            },
            web_search: WebSearchConfig {
                provider: config.web_search_provider,
                openai_api_key: config.legacy_web_search_credentials.openai.as_deref(),
                exa_api_key: config.legacy_web_search_credentials.exa.as_deref(),
                brave_api_key: config.legacy_web_search_credentials.brave.as_deref(),
            },
            behavior: BehaviorConfig {
                check_for_updates: config.check_for_updates,
                rtk: config.rtk,
                inline_shell: &config.inline_shell,
            },
            keybindings: &config.keybindings,
            prompt_templates: &config.prompt_templates,
        }
    }
}

impl Config {
    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(paths::rho_dir()?.join("config.toml"))
    }

    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        Self::load_with_store(path, &OsCredentialStore)
    }

    fn load_with_store(path: PathBuf, store: &dyn CredentialStore) -> anyhow::Result<Self> {
        if !path.exists() {
            Config::default().save_with_store(path.clone(), store)?;
        }

        let mut cfg = Config::default();
        let text = fs::read_to_string(&path)?;
        let file: PartialConfig = toml::from_str(&text)?;
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
        if let Some(v) = file.title_provider {
            cfg.title_provider = Some(v);
        }
        if let Some(v) = file.title_model {
            cfg.title_model = Some(v);
        }
        if let Some(v) = file.title_auth {
            cfg.title_auth = Some(v);
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
        }
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
        if let Some(group) = file.title {
            cfg.title_provider = group.provider.or(cfg.title_provider);
            cfg.title_model = group.model.or(cfg.title_model);
            cfg.title_auth = group.auth.or(cfg.title_auth);
        }
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
        if let Some(group) = file.behavior {
            cfg.check_for_updates = group.check_for_updates.unwrap_or(cfg.check_for_updates);
            cfg.rtk = group.rtk.unwrap_or(cfg.rtk);
            cfg.inline_shell = group
                .inline_shell
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(cfg.inline_shell);
        }
        if let Some(keybindings) = file.keybindings {
            cfg.keybindings = keybindings;
        }
        if matches!(cfg.migrate_legacy_web_search_credentials(store), Ok(true)) {
            let _cleanup_result = write_config(&path, &cfg);
        }
        Ok(cfg)
    }

    pub fn save(&self, path: Option<PathBuf>) -> anyhow::Result<()> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        self.save_with_store(path, &OsCredentialStore)
    }

    fn save_with_store(&self, path: PathBuf, store: &dyn CredentialStore) -> anyhow::Result<()> {
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

    fn migrate_legacy_web_search_credentials(
        &mut self,
        store: &dyn CredentialStore,
    ) -> crate::credentials::CredentialResult<bool> {
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
    web_search_openai_api_key: Option<String>,
    web_search_exa_api_key: Option<String>,
    web_search_brave_api_key: Option<String>,
    rtk: Option<bool>,
    inline_shell: Option<String>,
    display: Option<PartialDisplayConfig>,
    output: Option<PartialOutputConfig>,
    compaction: Option<PartialCompactionConfig>,
    title: Option<PartialTitleConfig>,
    web_search: Option<PartialWebSearchConfig>,
    behavior: Option<PartialBehaviorConfig>,
    keybindings: Option<Keybindings>,
    prompt_templates: Option<crate::prompt_templates::PromptTemplates>,
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
