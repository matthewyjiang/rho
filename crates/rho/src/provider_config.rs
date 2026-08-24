use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

use super::Config;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProviderConfigs {
    /// Set only after `/login ollama` or an explicit `[providers.ollama]` table.
    /// First-run config does not invent a default endpoint.
    pub(crate) ollama: Option<ProviderEndpointConfig>,
    pub(crate) custom: BTreeMap<String, ProviderEndpointConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderEndpointConfig {
    pub(crate) base_url: Url,
    /// models.dev provider slug whose catalog rows this host should borrow.
    pub(crate) catalog: Option<String>,
    /// How this host rematches models.dev rows. Default is slug-or-host.
    pub(crate) catalog_lookup: rho_providers::provider::CatalogLookupMode,
    /// Wire API this host speaks. Ollama is always Chat Completions.
    pub(crate) api: rho_providers::provider::OpenAiCompatibleApi,
}

impl ProviderConfigs {
    fn endpoint(&self, provider: &str) -> Option<&Url> {
        match provider {
            "ollama" => self.ollama.as_ref().map(|endpoint| &endpoint.base_url),
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
            self.ollama = Some(ProviderEndpointConfig {
                base_url: parsed,
                catalog: None,
                catalog_lookup: rho_providers::provider::CatalogLookupMode::Slug,
                api: rho_providers::provider::OpenAiCompatibleApi::ChatCompletions,
            });
            return Ok(());
        }
        rho_providers::provider::validate_custom_provider_name(provider)?;
        let existing = self.custom.get(provider);
        let catalog = existing.and_then(|endpoint| endpoint.catalog.clone());
        let catalog_lookup = existing
            .map(|endpoint| endpoint.catalog_lookup)
            .unwrap_or_default();
        let api = existing.map(|endpoint| endpoint.api).unwrap_or_default();
        self.custom.insert(
            provider.to_string(),
            ProviderEndpointConfig {
                base_url: parsed,
                catalog,
                catalog_lookup,
                api,
            },
        );
        Ok(())
    }

    fn set_catalog(&mut self, provider: &str, catalog: Option<String>) -> anyhow::Result<()> {
        let field = format!("providers.custom.{provider}.catalog");
        let catalog = match catalog {
            Some(value) => Some(parse_provider_catalog(&field, &value)?),
            None => None,
        };
        let Some(endpoint) = self.custom.get_mut(provider) else {
            anyhow::bail!("{field} requires a configured base_url");
        };
        if catalog.is_some()
            && endpoint.catalog_lookup == rho_providers::provider::CatalogLookupMode::ModelId
        {
            anyhow::bail!("{field} cannot be combined with catalog_mode = \"model-id\"");
        }
        endpoint.catalog = catalog;
        Ok(())
    }

    fn set_catalog_mode(
        &mut self,
        provider: &str,
        catalog_mode: Option<String>,
    ) -> anyhow::Result<()> {
        let field = format!("providers.custom.{provider}.catalog_mode");
        let catalog_lookup = match catalog_mode {
            Some(value) => parse_provider_catalog_mode(&field, &value)?,
            None => rho_providers::provider::CatalogLookupMode::Slug,
        };
        let Some(endpoint) = self.custom.get_mut(provider) else {
            anyhow::bail!("{field} requires a configured base_url");
        };
        if catalog_lookup == rho_providers::provider::CatalogLookupMode::ModelId
            && endpoint.catalog.is_some()
        {
            anyhow::bail!("{field} cannot be combined with catalog");
        }
        endpoint.catalog_lookup = catalog_lookup;
        Ok(())
    }

    fn set_api(&mut self, provider: &str, api: Option<String>) -> anyhow::Result<()> {
        let field = format!("providers.custom.{provider}.api");
        let api = match api {
            Some(value) => parse_provider_api(&field, &value)?,
            None => rho_providers::provider::OpenAiCompatibleApi::ChatCompletions,
        };
        self.set_openai_compatible_api(provider, api)
    }

    /// Writes the wire API after [`Self::set_endpoint`]. `/login` uses this so
    /// a new host is not stuck on Chat Completions when Responses was chosen.
    pub(crate) fn set_openai_compatible_api(
        &mut self,
        provider: &str,
        api: rho_providers::provider::OpenAiCompatibleApi,
    ) -> anyhow::Result<()> {
        let field = format!("providers.custom.{provider}.api");
        let Some(endpoint) = self.custom.get_mut(provider) else {
            anyhow::bail!("{field} requires a configured base_url");
        };
        endpoint.api = api;
        Ok(())
    }

    pub(super) fn apply(&mut self, partial: PartialProviderConfigs) -> anyhow::Result<()> {
        if let Some(endpoint) = partial.ollama {
            if endpoint.catalog.is_some() {
                anyhow::bail!("providers.ollama does not accept catalog");
            }
            if endpoint.catalog_mode.is_some() {
                anyhow::bail!("providers.ollama does not accept catalog_mode");
            }
            if endpoint.api.is_some() {
                anyhow::bail!("providers.ollama does not accept api");
            }
            if let Some(base_url) = endpoint.base_url {
                self.set_endpoint("ollama", &base_url)?;
            }
        }
        if let Some(custom) = partial.custom {
            self.custom.clear();
            for (name, endpoint) in custom {
                let Some(base_url) = endpoint.base_url else {
                    anyhow::bail!("providers.custom.{name} requires base_url");
                };
                self.set_endpoint(&name, &base_url)?;
                self.set_catalog(&name, endpoint.catalog)?;
                self.set_catalog_mode(&name, endpoint.catalog_mode)?;
                self.set_api(&name, endpoint.api)?;
            }
        }
        Ok(())
    }

    /// Each config-defined host paired with its intern options.
    fn specs(
        &self,
    ) -> impl Iterator<
        Item = (
            rho_providers::provider::CustomProviderSpec<'_>,
            rho_providers::provider::CustomProviderOptions,
        ),
    > {
        self.custom.iter().map(|(name, endpoint)| {
            (
                rho_providers::provider::CustomProviderSpec::new(name, endpoint.catalog.as_deref()),
                rho_providers::provider::CustomProviderOptions::new()
                    .with_catalog_lookup(endpoint.catalog_lookup)
                    .with_api(endpoint.api),
            )
        })
    }

    /// Interns config-defined hosts without changing the process-wide picker set.
    pub(crate) fn intern_names(&self) -> anyhow::Result<std::sync::Arc<[String]>> {
        rho_providers::provider::intern_custom_openai_compatible_providers_with_options(
            self.specs(),
        )
    }

    /// Publishes config-defined hosts as the process-wide named provider set.
    pub(crate) fn activate(&self) -> anyhow::Result<()> {
        rho_providers::provider::install_custom_openai_compatible_providers_with_options(
            self.specs(),
        )
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

fn parse_provider_catalog(field: &str, catalog: &str) -> anyhow::Result<String> {
    let catalog = catalog.trim();
    if catalog.is_empty() {
        anyhow::bail!("{field} must not be empty");
    }
    if catalog.contains('/') {
        anyhow::bail!(
            "{field} must be a models.dev provider slug; set a per-model catalog in models.toml"
        );
    }
    if catalog.contains(',') {
        anyhow::bail!("{field} must not contain ','");
    }
    if catalog.chars().any(char::is_whitespace) {
        anyhow::bail!("{field} must not contain whitespace");
    }
    Ok(catalog.to_string())
}

fn parse_provider_catalog_mode(
    field: &str,
    catalog_mode: &str,
) -> anyhow::Result<rho_providers::provider::CatalogLookupMode> {
    catalog_mode
        .parse()
        .map_err(|error| anyhow::anyhow!("{field} {error}"))
}

fn parse_provider_api(
    field: &str,
    api: &str,
) -> anyhow::Result<rho_providers::provider::OpenAiCompatibleApi> {
    api.parse()
        .map_err(|error| anyhow::anyhow!("{field} {error}"))
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

    /// Promotes a custom host from `none` to `{name}-api-key` when a key is stored.
    ///
    /// Login can store the secret without rewriting `auth`. Keyless is the
    /// default, so restart would keep sending no `Authorization` header unless
    /// this runs. One-directional: a keyed profile is never written back to
    /// `none`, including when the credential store errors or is empty.
    pub(crate) fn promote_stored_custom_auth(
        &mut self,
        store: &dyn rho_providers::credentials::CredentialStore,
    ) -> bool {
        if self.auth != rho_providers::provider::KEYLESS_AUTH {
            return false;
        }
        let Some(descriptor) = rho_providers::provider::interned_custom_provider(&self.provider)
        else {
            return false;
        };
        let selected = descriptor.discovery_auth(store).id;
        if selected == rho_providers::provider::KEYLESS_AUTH {
            return false;
        }
        self.auth = selected.to_string();
        true
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
    #[serde(skip_serializing_if = "Option::is_none")]
    ollama: Option<PersistedEndpointConfig<'a>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    custom: BTreeMap<&'a str, PersistedEndpointConfig<'a>>,
}

impl PersistedProviderConfigs<'_> {
    pub(super) fn is_empty(&self) -> bool {
        self.ollama.is_none() && self.custom.is_empty()
    }
}

#[derive(Serialize)]
struct PersistedEndpointConfig<'a> {
    base_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api: Option<&'static str>,
}

impl<'a> From<&'a ProviderConfigs> for PersistedProviderConfigs<'a> {
    fn from(config: &'a ProviderConfigs) -> Self {
        Self {
            ollama: config
                .ollama
                .as_ref()
                .map(|endpoint| PersistedEndpointConfig {
                    base_url: endpoint.base_url.as_str(),
                    catalog: None,
                    catalog_mode: None,
                    api: None,
                }),
            custom: config
                .custom
                .iter()
                .map(|(name, endpoint)| {
                    (
                        name.as_str(),
                        PersistedEndpointConfig {
                            base_url: endpoint.base_url.as_str(),
                            catalog: endpoint.catalog.as_deref(),
                            catalog_mode: persisted_catalog_mode(endpoint.catalog_lookup),
                            api: persisted_api(endpoint.api),
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

fn persisted_catalog_mode(
    lookup: rho_providers::provider::CatalogLookupMode,
) -> Option<&'static str> {
    match lookup {
        rho_providers::provider::CatalogLookupMode::Slug => None,
        rho_providers::provider::CatalogLookupMode::ModelId => Some(lookup.as_str()),
    }
}

fn persisted_api(api: rho_providers::provider::OpenAiCompatibleApi) -> Option<&'static str> {
    match api {
        rho_providers::provider::OpenAiCompatibleApi::ChatCompletions => None,
        rho_providers::provider::OpenAiCompatibleApi::Responses => Some(api.as_str()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PartialEndpointConfig {
    pub(super) base_url: Option<String>,
    pub(super) catalog: Option<String>,
    pub(super) catalog_mode: Option<String>,
    pub(super) api: Option<String>,
}

#[cfg(test)]
#[path = "provider_config_tests.rs"]
mod tests;
