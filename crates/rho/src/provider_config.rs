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
    /// Whether this provider stores a configurable base URL.
    ///
    /// Keep in sync with `endpoint` and `set_endpoint` below.
    pub(crate) fn stores_endpoint(provider: &str) -> bool {
        matches!(provider, "ollama" | "qwen-token-plan")
    }

    fn endpoint(&self, provider: &str) -> Option<&Url> {
        match provider {
            "ollama" => Some(&self.ollama.base_url),
            "qwen-token-plan" => Some(&self.qwen_token_plan.base_url),
            _ => None,
        }
    }

    /// Validates and stores a provider base URL. This is the one write path
    /// shared by config loading and interactive login.
    pub(crate) fn set_endpoint(&mut self, provider: &str, base_url: &str) -> anyhow::Result<()> {
        let field = format!("providers.{provider}.base_url");
        let parsed = parse_provider_base_url(&field, base_url)?;
        let slot = match provider {
            "ollama" => &mut self.ollama.base_url,
            "qwen-token-plan" => &mut self.qwen_token_plan.base_url,
            _ => anyhow::bail!("provider '{provider}' has no configurable base URL"),
        };
        *slot = parsed;
        Ok(())
    }

    pub(super) fn apply(&mut self, partial: PartialProviderConfigs) -> anyhow::Result<()> {
        if let Some(base_url) = partial.ollama.and_then(|ollama| ollama.base_url) {
            self.set_endpoint("ollama", &base_url)?;
        }
        if let Some(base_url) = partial
            .qwen_token_plan
            .and_then(|qwen_token_plan| qwen_token_plan.base_url)
        {
            self.set_endpoint("qwen-token-plan", &base_url)?;
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
