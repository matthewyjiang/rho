use std::{fs, path::PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub model: String,
    pub api_base: String,
    pub max_steps: usize,
    pub max_output_bytes: usize,
    pub cwd: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "gpt-4.1-mini".into(),
            api_base: "https://api.openai.com/v1".into(),
            max_steps: 8,
            max_output_bytes: 12000,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let mut cfg = Config::default();
        if let Some(path) = path {
            let text = fs::read_to_string(path)?;
            let file: PartialConfig = toml::from_str(&text)?;
            if let Some(v) = file.model {
                cfg.model = v;
            }
            if let Some(v) = file.api_base {
                cfg.api_base = v;
            }
            if let Some(v) = file.max_steps {
                cfg.max_steps = v;
            }
            if let Some(v) = file.max_output_bytes {
                cfg.max_output_bytes = v;
            }
            if let Some(v) = file.cwd {
                cfg.cwd = v;
            }
        }
        Ok(cfg)
    }
}

#[derive(Deserialize)]
struct PartialConfig {
    model: Option<String>,
    api_base: Option<String>,
    max_steps: Option<usize>,
    max_output_bytes: Option<usize>,
    cwd: Option<PathBuf>,
}
