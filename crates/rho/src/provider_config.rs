use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

use super::Config;

pub(crate) const DEFAULT_OLLAMA_BASE_URL: &str = rho_providers::model::registry::OLLAMA_API_BASE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderConfigs {
    pub(crate) ollama: ProviderEndpointConfig,
    pub(crate) custom: BTreeMap<String, ProviderEndpointConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderEndpointConfig {
    pub(crate) base_url: Url,
}

impl Default for ProviderConfigs {
    fn default() -> Self {
        Self {
            ollama: ProviderEndpointConfig {
                base_url: Url::parse(DEFAULT_OLLAMA_BASE_URL)
                    .expect("the default Ollama API base must be a valid URL"),
            },
            custom: BTreeMap::new(),
        }
    }
}

impl ProviderConfigs {
    fn endpoint(&self, provider: &str) -> Option<&Url> {
        match provider {
            "ollama" => Some(&self.ollama.base_url),
            name => self.custom.get(name).map(|endpoint| &endpoint.base_url),
        }
    }

    /// Validates and stores a provider base URL. This is the one write path
    /// shared by config loading.
    pub(crate) fn set_endpoint(&mut self, provider: &str, base_url: &str) -> anyhow::Result<()> {
        let field = if provider == "ollama" {
            format!("providers.{provider}.base_url")
        } else {
            format!("providers.custom.{provider}.base_url")
        };
        let parsed = parse_provider_base_url(&field, base_url)?;
        if provider == "ollama" {
            self.ollama.base_url = parsed;
            return Ok(());
        }
        rho_providers::provider::validate_custom_provider_name(provider)?;
        self.custom.insert(
            provider.to_string(),
            ProviderEndpointConfig { base_url: parsed },
        );
        Ok(())
    }

    pub(super) fn apply(&mut self, partial: PartialProviderConfigs) -> anyhow::Result<()> {
        if let Some(base_url) = partial.ollama.and_then(|endpoint| endpoint.base_url) {
            self.set_endpoint("ollama", &base_url)?;
        }
        if let Some(custom) = partial.custom {
            self.custom.clear();
            for (name, endpoint) in custom {
                let Some(base_url) = endpoint.base_url else {
                    anyhow::bail!("providers.custom.{name} requires base_url");
                };
                self.set_endpoint(&name, &base_url)?;
            }
        }
        Ok(())
    }

    /// Interns config-defined hosts without changing the process-wide picker set.
    pub(crate) fn intern_names(&self) -> anyhow::Result<std::sync::Arc<[String]>> {
        rho_providers::provider::intern_custom_openai_compatible_providers(
            self.custom.keys().map(String::as_str),
        )
    }

    /// Publishes config-defined hosts as the process-wide named provider set.
    pub(crate) fn activate(&self) -> anyhow::Result<()> {
        rho_providers::provider::install_custom_openai_compatible_providers(
            self.custom.keys().map(String::as_str),
        )
    }

    /// Refreshes this thread's overlay so a newly written host is visible now.
    pub(crate) fn refresh_thread_visibility(&self) -> anyhow::Result<()> {
        rho_providers::provider::replace_current_thread_custom_providers(self.intern_names()?);
        Ok(())
    }

    /// Interns this config's hosts and overlays them on the current thread.
    pub(crate) fn thread_scope(
        &self,
    ) -> anyhow::Result<rho_providers::provider::CustomProviderThreadScope> {
        Ok(rho_providers::provider::CustomProviderThreadScope::enter(
            self.intern_names()?,
        ))
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
        // Auth ids such as `{name}-api-key` exist only after intern.
        let _ = self.providers.intern_names()?;
        normalize_selection(
            &self.providers,
            &mut self.provider,
            &mut self.auth,
            &mut self.model,
            None,
        )?;
        // Delegating selections have no Rho provider or auth to normalize; the
        // claude binary owns both.
        for (id, selection) in &mut self.internal_agents {
            match &mut selection.target {
                crate::config::InternalAgentTarget::Rho(rho) => {
                    normalize_selection(
                        &self.providers,
                        &mut rho.provider,
                        &mut rho.auth,
                        &mut rho.model,
                        Some(id.as_str()),
                    )?;
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

fn normalize_selection(
    providers: &ProviderConfigs,
    provider: &mut String,
    auth: &mut String,
    model: &mut String,
    internal_agent: Option<&str>,
) -> anyhow::Result<()> {
    if providers.custom.contains_key(provider.as_str()) {
        let accepted = rho_providers::provider::interned_custom_provider(provider)
            .is_some_and(|descriptor| descriptor.auth_mode(auth).is_some());
        if !accepted {
            *auth = "none".into();
        }
        return Ok(());
    }
    let profile = rho_providers::provider::resolve_profile(provider, auth).map_err(|error| {
        match internal_agent {
            Some(id) => anyhow::anyhow!("internal agent '{id}': {error}"),
            None => anyhow::anyhow!("{error}"),
        }
    })?;
    *provider = profile.provider_name().into();
    *auth = profile.auth_id().into();
    // Collapse legacy wire ids (for example poolside/laguna-m.1) to the
    // internal model id used by cache, config, and display joins.
    *model = profile.provider.canonicalize_model_id(model);
    Ok(())
}

#[derive(Serialize)]
pub(super) struct PersistedProviderConfigs<'a> {
    ollama: PersistedEndpointConfig<'a>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    custom: BTreeMap<&'a str, PersistedEndpointConfig<'a>>,
}

#[derive(Serialize)]
struct PersistedEndpointConfig<'a> {
    base_url: &'a str,
}

impl<'a> From<&'a ProviderConfigs> for PersistedProviderConfigs<'a> {
    fn from(config: &'a ProviderConfigs) -> Self {
        Self {
            ollama: PersistedEndpointConfig {
                base_url: config.ollama.base_url.as_str(),
            },
            custom: config
                .custom
                .iter()
                .map(|(name, endpoint)| {
                    (
                        name.as_str(),
                        PersistedEndpointConfig {
                            base_url: endpoint.base_url.as_str(),
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialProviderConfigs {
    pub(super) ollama: Option<PartialEndpointConfig>,
    pub(super) custom: Option<BTreeMap<String, PartialEndpointConfig>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialEndpointConfig {
    pub(super) base_url: Option<String>,
}

#[cfg(test)]
#[path = "provider_config_tests.rs"]
mod tests;
