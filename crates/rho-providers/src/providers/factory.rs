use std::{fmt, sync::Arc};

use rho_sdk::ProviderError;

use crate::{
    auth::provider_credentials::ProviderCredentialSource,
    model::ModelError,
    providers::builder::{ProviderBuildOptions, ProviderBuilder, ProviderCredential},
    reasoning::ReasoningLevel,
};

/// Builds a provider from side-effect-free options and explicit credentials.
pub fn build_sdk_provider_explicit(
    options: ProviderBuildOptions,
    credential: ProviderCredential,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, ModelError> {
    ProviderBuilder::new(options, credential).build()
}

/// Acquires credentials through an explicitly selected application adapter and
/// passes them to side-effect-free provider construction.
pub fn build_sdk_provider_with_source(
    options: ProviderBuildOptions,
    credentials: &dyn ProviderCredentialSource,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, ModelError> {
    #[cfg(debug_assertions)]
    if let Some(provider) = super::tui_fixture::from_env(options.provider(), options.model())
        .map_err(ModelError::InvalidResponse)?
    {
        return Ok(provider);
    }

    let credential = credentials.acquire(options.provider())?;
    build_sdk_provider_explicit(options, credential)
}

pub fn build_automation_provider(
    options: ProviderBuildOptions,
    credentials: &dyn ProviderCredentialSource,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, ModelError> {
    #[cfg(debug_assertions)]
    if let Some(provider) = super::automation_fixture::from_env(options.provider(), options.model())
        .map_err(ModelError::InvalidResponse)?
    {
        return Ok(provider);
    }

    build_sdk_provider_with_source(options, credentials)
}

/// Builds a provider from provider/model/reasoning and an explicit credential source.
///
/// The providers crate does not select a credential store. Callers must pass a
/// [`ProviderCredentialSource`] (for example an application adapter over
/// [`crate::OsCredentialStore`] or [`crate::FileCredentialStore`]).
pub fn build_sdk_provider(
    provider: &str,
    model: &str,
    reasoning: ReasoningLevel,
    credentials: &dyn ProviderCredentialSource,
) -> Result<Arc<dyn rho_sdk::provider::ModelProvider>, ModelError> {
    let options = ProviderBuildOptions::new(provider, model, reasoning)?;
    build_sdk_provider_with_source(options, credentials)
}

/// Provider stub that always fails with a sanitized, cloneable error.
///
/// Construction converts the source [`ModelError`] once so the runtime does not
/// need a hand-maintained clone ladder for every credential variant.
#[derive(Debug)]
pub struct UnavailableProvider {
    error: ProviderError,
    message: String,
}

impl UnavailableProvider {
    pub fn new(error: ModelError) -> Self {
        let message = error.to_string();
        Self {
            error: super::sdk_contract::provider_error_from_model_error(error),
            message,
        }
    }
}

impl rho_sdk::provider::ModelProvider for UnavailableProvider {
    fn identity(&self) -> rho_sdk::model::ModelIdentity {
        rho_sdk::model::ModelIdentity::new("unavailable", "unavailable", "unavailable")
    }

    fn send_turn<'a>(
        &'a self,
        _request: rho_sdk::model::ModelRequest<'a>,
    ) -> rho_sdk::provider::ProviderFuture<'a> {
        let error = self.error.clone();
        Box::pin(async move { Err(error) })
    }
}

impl fmt::Display for UnavailableProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}
