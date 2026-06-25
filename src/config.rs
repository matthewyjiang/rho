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
    pub compact_recent_messages: usize,
    pub title_provider: Option<String>,
    pub title_model: Option<String>,
    pub title_auth: Option<String>,
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
            compact_recent_messages: 8,
            title_provider: None,
            title_model: None,
            title_auth: None,
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
            cfg.compact_threshold_percent = v.clamp(1, 100);
        }
        if let Some(v) = file.compact_recent_messages {
            cfg.compact_recent_messages = v.max(1);
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
        Ok(cfg)
    }

    pub fn save(&self, path: Option<PathBuf>) -> anyhow::Result<()> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

impl From<&Config> for CompactionConfig {
    fn from(config: &Config) -> Self {
        Self {
            auto_compact: config.auto_compact,
            threshold_percent: config.compact_threshold_percent,
            recent_messages: config.compact_recent_messages,
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
    compact_recent_messages: Option<usize>,
    title_provider: Option<String>,
    title_model: Option<String>,
    title_auth: Option<String>,
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
}
