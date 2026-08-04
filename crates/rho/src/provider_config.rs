use serde::{Deserialize, Serialize};
use url::Url;

use super::Config;

pub(crate) const DEFAULT_OLLAMA_BASE_URL: &str = rho_providers::model::registry::OLLAMA_API_BASE;
pub(crate) const DEFAULT_QWEN_TOKEN_PLAN_BASE_URL: &str =
    rho_providers::model::registry::QWEN_TOKEN_PLAN_API_BASE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderConfigs {
    pub(crate) ollama: OllamaProviderConfig,
    pub(crate) qwen_token_plan: QwenTokenPlanProviderConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OllamaProviderConfig {
    pub(crate) base_url: Url,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QwenTokenPlanProviderConfig {
    pub(crate) base_url: Url,
}

impl Default for ProviderConfigs {
    fn default() -> Self {
        Self {
            ollama: OllamaProviderConfig {
                base_url: Url::parse(DEFAULT_OLLAMA_BASE_URL)
                    .expect("the default Ollama API base must be a valid URL"),
            },
            qwen_token_plan: QwenTokenPlanProviderConfig {
                base_url: Url::parse(DEFAULT_QWEN_TOKEN_PLAN_BASE_URL)
                    .expect("the default Qwen Token Plan API base must be a valid URL"),
            },
        }
    }
}

impl ProviderConfigs {
    fn endpoint(&self, provider: &str) -> Option<&Url> {
        match provider {
            "ollama" => Some(&self.ollama.base_url),
            "qwen-token-plan" => Some(&self.qwen_token_plan.base_url),
            _ => None,
        }
    }

    pub(super) fn apply(&mut self, partial: PartialProviderConfigs) -> anyhow::Result<()> {
        if let Some(ollama) = partial.ollama {
            if let Some(base_url) = ollama.base_url {
                self.ollama.base_url =
                    parse_provider_base_url("providers.ollama.base_url", &base_url)?;
            }
        }
        if let Some(qwen_token_plan) = partial.qwen_token_plan {
            if let Some(base_url) = qwen_token_plan.base_url {
                self.qwen_token_plan.base_url =
                    parse_provider_base_url("providers.qwen-token-plan.base_url", &base_url)?;
            }
        }
        Ok(())
    }
}

fn parse_provider_base_url(field: &str, base_url: &str) -> anyhow::Result<Url> {
    let parsed =
        Url::parse(base_url).map_err(|error| anyhow::anyhow!("invalid {field}: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("{field} must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("{field} must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        anyhow::bail!("{field} must not contain a query or fragment");
    }
    Ok(parsed)
}

impl Config {
    pub(crate) fn normalize_provider_profiles(&mut self) -> anyhow::Result<()> {
        let profile = rho_providers::provider::resolve_profile(&self.provider, &self.auth)?;
        self.provider = profile.provider_name().into();
        self.auth = profile.auth_id().into();
        // Collapse legacy wire ids (for example poolside/laguna-m.1) to the
        // internal model id used by cache, config, and display joins.
        self.model = profile.provider.canonicalize_model_id(&self.model);
        for (id, selection) in &mut self.internal_agents {
            let profile =
                rho_providers::provider::resolve_profile(&selection.provider, &selection.auth)
                    .map_err(|error| anyhow::anyhow!("internal agent '{id}': {error}"))?;
            selection.provider = profile.provider_name().into();
            selection.auth = profile.auth_id().into();
            selection.model = profile.provider.canonicalize_model_id(&selection.model);
        }
        Ok(())
    }

    /// Resolves the one API base shared by runtime requests, model discovery, and diagnostics.
    pub(crate) fn resolved_provider_endpoint(&self, provider: &str) -> Option<Url> {
        self.providers.endpoint(provider).cloned().or_else(|| {
            match rho_providers::model::registry::provider_runtime(provider) {
                Some(rho_providers::model::registry::ProviderRuntime::OpenAiCompatible {
                    default_api_base,
                    ..
                }) => Some(
                    Url::parse(default_api_base)
                        .expect("built-in provider API bases must be valid URLs"),
                ),
                _ => None,
            }
        })
    }
}

#[derive(Serialize)]
pub(super) struct PersistedProviderConfigs<'a> {
    ollama: PersistedOllamaProviderConfig<'a>,
    #[serde(rename = "qwen-token-plan")]
    qwen_token_plan: PersistedQwenTokenPlanProviderConfig<'a>,
}

#[derive(Serialize)]
struct PersistedOllamaProviderConfig<'a> {
    base_url: &'a str,
}

#[derive(Serialize)]
struct PersistedQwenTokenPlanProviderConfig<'a> {
    base_url: &'a str,
}

impl<'a> From<&'a ProviderConfigs> for PersistedProviderConfigs<'a> {
    fn from(config: &'a ProviderConfigs) -> Self {
        Self {
            ollama: PersistedOllamaProviderConfig {
                base_url: config.ollama.base_url.as_str(),
            },
            qwen_token_plan: PersistedQwenTokenPlanProviderConfig {
                base_url: config.qwen_token_plan.base_url.as_str(),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialProviderConfigs {
    pub(super) ollama: Option<PartialOllamaProviderConfig>,
    #[serde(rename = "qwen-token-plan")]
    pub(super) qwen_token_plan: Option<PartialQwenTokenPlanProviderConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialOllamaProviderConfig {
    pub(super) base_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialQwenTokenPlanProviderConfig {
    pub(super) base_url: Option<String>,
}

#[cfg(test)]
#[path = "provider_config_tests.rs"]
mod tests;
