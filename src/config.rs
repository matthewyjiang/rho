use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub provider: String,
    pub model: String,
    pub max_output_bytes: usize,
    pub auth: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            max_output_bytes: 12000,
            auth: "api-key".into(),
        }
    }
}

impl Config {
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        Ok(home.join(".rho").join("config.toml"))
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
        if let Some(v) = file.auth {
            cfg.auth = v;
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

#[derive(Deserialize)]
struct PartialConfig {
    provider: Option<String>,
    model: Option<String>,
    max_output_bytes: Option<usize>,
    auth: Option<String>,
}
