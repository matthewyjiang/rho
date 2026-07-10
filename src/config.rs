use std::{fmt, fs, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{
    agent::CompactionConfig,
    credentials::{
        load_web_search_api_key, save_web_search_api_key, CredentialStore, OsCredentialStore,
        WebSearchCredential,
    },
    model::favorites::{favorite_model_values, normalized_favorite_models},
    paths,
    reasoning::ReasoningLevel,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    #[serde(flatten)]
    pub(crate) legacy_web_search_credentials: LegacyWebSearchCredentials,
    pub rtk: bool,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
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
    fs::write(path, toml::to_string_pretty(config)?)?;
    Ok(())
}

impl Config {
    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(paths::rho_dir()?.join("config.toml"))
    }

    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        if !path.exists() {
            Config::default().save(Some(path.clone()))?;
        }

        let mut cfg = Config::default();
        let text = fs::read_to_string(&path)?;
        let file: PartialConfig = toml::from_str(&text)?;
        if let Some(v) = file.provider {
            cfg.provider = v;
        }
        if let Some(v) = file.model {
            cfg.model = v;
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
        let store = OsCredentialStore;
        if matches!(cfg.migrate_legacy_web_search_credentials(&store), Ok(true)) {
            write_config(&path, &cfg)?;
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
    model: Option<String>,
    max_output_bytes: Option<usize>,
    max_tool_output_lines: Option<usize>,
    auth: Option<String>,
    reasoning: Option<ReasoningLevel>,
    reasoning_effort: Option<String>,
    show_reasoning_output: Option<bool>,
    auto_compact: Option<bool>,
    compact_threshold_percent: Option<u8>,
    compact_target_percent: Option<u8>,
    #[allow(dead_code)]
    compact_recent_messages: Option<usize>,
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
mod tests {
    use super::Config;

    #[test]
    fn default_shows_reasoning_output() {
        assert!(Config::default().show_reasoning_output);
    }

    #[test]
    fn loads_reasoning_output_visibility() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "show_reasoning_output = false\n").unwrap();

        let config = Config::load(Some(path)).unwrap();

        assert!(!config.show_reasoning_output);
    }

    #[test]
    fn loads_check_for_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "check_for_updates = false\n").unwrap();

        let config = Config::load(Some(path)).unwrap();

        assert!(!config.check_for_updates);
    }

    #[test]
    fn loads_and_normalizes_compaction_percentages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "auto_compact = true\ncompact_threshold_percent = 80\ncompact_target_percent = 95\n",
        )
        .unwrap();

        let config = Config::load(Some(path)).unwrap();

        assert!(config.auto_compact);
        assert_eq!(config.compact_threshold_percent, 80);
        assert_eq!(config.compact_target_percent, 79);
    }

    #[test]
    fn loads_rtk_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "rtk = false\n").unwrap();

        let config = Config::load(Some(path)).unwrap();

        assert!(!config.rtk);
    }

    #[test]
    fn loads_and_saves_favorite_models() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
favorite_models = [
  " openai/gpt-5.5 ",
  "missing-separator",
  "openai/gpt-5.5",
  "anthropic/claude-sonnet-4-5",
]
"#,
        )
        .unwrap();

        let config = Config::load(Some(path.clone())).unwrap();

        assert_eq!(
            config.favorite_models,
            vec!["openai/gpt-5.5", "anthropic/claude-sonnet-4-5"]
        );

        config.save(Some(path.clone())).unwrap();
        let saved = std::fs::read_to_string(path).unwrap();
        assert!(saved.contains("favorite_models"), "{saved}");
        assert!(saved.contains("openai/gpt-5.5"), "{saved}");
        assert!(!saved.contains("missing-separator"), "{saved}");
    }

    #[test]
    fn unsupported_web_search_config_providers_fall_back_to_auto() {
        for provider in ["parallel", "tavily", "perplexity", "gemini", "unknown"] {
            assert_eq!(
                super::SearchProvider::from_config_value(provider),
                super::SearchProvider::Auto
            );
        }
    }

    #[test]
    fn supported_web_search_config_provider_is_preserved() {
        assert_eq!(
            super::SearchProvider::from_config_value(" brave "),
            super::SearchProvider::Brave
        );
    }

    #[test]
    fn save_preserves_legacy_web_search_keys_when_credentials_are_unavailable() {
        struct UnavailableCredentialStore;

        impl crate::credentials::CredentialStore for UnavailableCredentialStore {
            fn get_secret(
                &self,
                _account: &str,
            ) -> crate::credentials::CredentialResult<Option<String>> {
                Err(crate::credentials::CredentialError::StoreUnavailable(
                    "test store unavailable".into(),
                ))
            }

            fn set_secret(
                &self,
                _account: &str,
                _secret: &str,
            ) -> crate::credentials::CredentialResult<()> {
                Err(crate::credentials::CredentialError::StoreUnavailable(
                    "test store unavailable".into(),
                ))
            }

            fn delete_secret(&self, _account: &str) -> crate::credentials::CredentialResult<bool> {
                Err(crate::credentials::CredentialError::StoreUnavailable(
                    "test store unavailable".into(),
                ))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            rtk: false,
            legacy_web_search_credentials: super::LegacyWebSearchCredentials {
                openai: Some("sk-test".into()),
                exa: None,
                brave: None,
            },
            ..Config::default()
        };

        config
            .save_with_store(path.clone(), &UnavailableCredentialStore)
            .unwrap();

        let saved = std::fs::read_to_string(path).unwrap();
        assert!(
            saved.contains("web_search_openai_api_key = \"sk-test\""),
            "{saved}"
        );
        assert!(saved.contains("rtk = false"), "{saved}");
    }

    #[test]
    fn migrates_legacy_web_search_keys_to_credentials() {
        let store = crate::credentials::MemoryCredentialStore::default();
        let mut config = Config {
            legacy_web_search_credentials: super::LegacyWebSearchCredentials {
                openai: Some("sk-test".into()),
                exa: Some("exa-test".into()),
                brave: Some("BSA-test".into()),
            },
            ..Config::default()
        };

        assert!(config
            .migrate_legacy_web_search_credentials(&store)
            .unwrap());
        assert_eq!(
            crate::credentials::load_web_search_api_key(
                &store,
                crate::credentials::WebSearchCredential::OpenAi
            )
            .unwrap()
            .as_deref(),
            Some("sk-test")
        );
        assert_eq!(
            config.legacy_web_search_api_key(crate::credentials::WebSearchCredential::OpenAi),
            None
        );
    }

    #[test]
    fn saved_config_omits_migrated_web_search_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config::default();

        super::write_config(&path, &config).unwrap();

        let saved = std::fs::read_to_string(path).unwrap();
        assert!(!saved.contains("web_search_openai_api_key"), "{saved}");
        assert!(!saved.contains("web_search_exa_api_key"), "{saved}");
        assert!(!saved.contains("web_search_brave_api_key"), "{saved}");
    }
}
