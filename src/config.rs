use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{agent::CompactionConfig, paths, reasoning::ReasoningLevel};

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
    pub web_search_provider: String,
    pub check_for_updates: bool,
    pub web_search_openai_api_key: Option<String>,
    pub web_search_exa_api_key: Option<String>,
    pub web_search_brave_api_key: Option<String>,
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
            web_search_provider: "auto".into(),
            check_for_updates: true,
            web_search_openai_api_key: None,
            web_search_exa_api_key: None,
            web_search_brave_api_key: None,
            rtk: true,
        }
    }
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
        let text = fs::read_to_string(path)?;
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
        if let Some(v) = file.web_search_provider {
            cfg.web_search_provider = normalize_web_search_provider(v);
        }
        if let Some(v) = file.check_for_updates {
            cfg.check_for_updates = v;
        }
        if let Some(v) = file.web_search_openai_api_key {
            cfg.web_search_openai_api_key = non_empty_secret(v);
        }
        if let Some(v) = file.web_search_exa_api_key {
            cfg.web_search_exa_api_key = non_empty_secret(v);
        }
        if let Some(v) = file.web_search_brave_api_key {
            cfg.web_search_brave_api_key = non_empty_secret(v);
        }
        if let Some(v) = file.rtk {
            cfg.rtk = v;
        }
        Ok(cfg)
    }

    pub fn save(&self, path: Option<PathBuf>) -> anyhow::Result<()> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut config = self.clone();
        config.normalize_compaction_percentages();
        fs::write(path, toml::to_string_pretty(&config)?)?;
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
    web_search_provider: Option<String>,
    check_for_updates: Option<bool>,
    web_search_openai_api_key: Option<String>,
    web_search_exa_api_key: Option<String>,
    web_search_brave_api_key: Option<String>,
    rtk: Option<bool>,
}

fn normalize_web_search_provider(provider: String) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "auto" | "openai" | "exa" | "brave" | "disabled" => provider.trim().to_ascii_lowercase(),
        _ => "auto".into(),
    }
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
    fn loads_web_search_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
web_search_provider = "exa"
web_search_openai_api_key = " sk-test "
web_search_exa_api_key = "exa-test"
web_search_brave_api_key = "BSA-test"
"#,
        )
        .unwrap();

        let config = Config::load(Some(path)).unwrap();

        assert_eq!(config.web_search_provider, "exa");
        assert_eq!(config.web_search_openai_api_key.as_deref(), Some("sk-test"));
        assert_eq!(config.web_search_exa_api_key.as_deref(), Some("exa-test"));
        assert_eq!(config.web_search_brave_api_key.as_deref(), Some("BSA-test"));
    }
}
