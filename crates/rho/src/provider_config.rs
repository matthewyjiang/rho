use serde::{Deserialize, Serialize};
use url::Url;

use super::Config;

pub(crate) const DEFAULT_OLLAMA_BASE_URL: &str = rho_providers::model::registry::OLLAMA_API_BASE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderConfigs {
    pub(crate) ollama: OllamaProviderConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OllamaProviderConfig {
    pub(crate) base_url: Url,
}

impl Default for ProviderConfigs {
    fn default() -> Self {
        Self {
            ollama: OllamaProviderConfig {
                base_url: Url::parse(DEFAULT_OLLAMA_BASE_URL)
                    .expect("the default Ollama API base must be a valid URL"),
            },
        }
    }
}

impl ProviderConfigs {
    fn endpoint(&self, provider: &str) -> Option<&Url> {
        match provider {
            "ollama" => Some(&self.ollama.base_url),
            _ => None,
        }
    }

    /// Validates and stores a provider base URL. This is the one write path
    /// shared by config loading.
    pub(crate) fn set_endpoint(&mut self, provider: &str, base_url: &str) -> anyhow::Result<()> {
        let field = format!("providers.{provider}.base_url");
        let parsed = parse_provider_base_url(&field, base_url)?;
        let slot = match provider {
            "ollama" => &mut self.ollama.base_url,
            _ => anyhow::bail!("provider '{provider}' has no configurable base URL"),
        };
        *slot = parsed;
        Ok(())
    }

    pub(super) fn apply(&mut self, partial: PartialProviderConfigs) -> anyhow::Result<()> {
        if let Some(base_url) = partial.ollama.and_then(|ollama| ollama.base_url) {
            self.set_endpoint("ollama", &base_url)?;
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
        // Delegating selections have no Rho provider or auth to normalize; the
        // claude binary owns both.
        for (id, selection) in &mut self.internal_agents {
            match &mut selection.target {
                crate::config::InternalAgentTarget::Rho(rho) => {
                    let profile =
                        rho_providers::provider::resolve_profile(&rho.provider, &rho.auth)
                            .map_err(|error| anyhow::anyhow!("internal agent '{id}': {error}"))?;
                    rho.provider = profile.provider_name().into();
                    rho.auth = profile.auth_id().into();
                    rho.model = profile.provider.canonicalize_model_id(&rho.model);
                }
                crate::config::InternalAgentTarget::ClaudeCli { .. } => {}
            }
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
}

#[derive(Serialize)]
struct PersistedOllamaProviderConfig<'a> {
    base_url: &'a str,
}

impl<'a> From<&'a ProviderConfigs> for PersistedProviderConfigs<'a> {
    fn from(config: &'a ProviderConfigs) -> Self {
        Self {
            ollama: PersistedOllamaProviderConfig {
                base_url: config.ollama.base_url.as_str(),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialProviderConfigs {
    pub(super) ollama: Option<PartialOllamaProviderConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialOllamaProviderConfig {
    pub(super) base_url: Option<String>,
}

#[cfg(test)]
#[path = "provider_config_tests.rs"]
mod tests;
