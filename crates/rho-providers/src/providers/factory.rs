use std::{fmt, sync::Arc};

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

#[derive(Debug)]
pub struct UnavailableProvider {
    error: ModelError,
}

impl UnavailableProvider {
    pub fn new(error: ModelError) -> Self {
        Self { error }
    }
}

fn clone_model_error(error: &ModelError) -> ModelError {
    match error {
        ModelError::MissingApiKey => ModelError::MissingApiKey,
        ModelError::MissingCodexAuth => ModelError::MissingCodexAuth,
        ModelError::MissingAnthropicApiKey => ModelError::MissingAnthropicApiKey,
        ModelError::MissingGoogleApiKey => ModelError::MissingGoogleApiKey,
        ModelError::MissingGithubCopilotAuth => ModelError::MissingGithubCopilotAuth,
        ModelError::MissingXaiApiKey => ModelError::MissingXaiApiKey,
        ModelError::MissingXaiAuth => ModelError::MissingXaiAuth,
        ModelError::MissingMoonshotApiKey => ModelError::MissingMoonshotApiKey,
        ModelError::MissingOpenRouterApiKey => ModelError::MissingOpenRouterApiKey,
        ModelError::MissingCredentialProfile(message) => {
            ModelError::MissingCredentialProfile(message)
        }
        ModelError::MissingKimiAuth => ModelError::MissingKimiAuth,
        ModelError::Credentials(err) => ModelError::Credentials(err.clone()),
        ModelError::UnsupportedReasoning {
            provider,
            model,
            requested,
        } => ModelError::UnsupportedReasoning {
            provider,
            model: model.clone(),
            requested: *requested,
        },
        ModelError::UnsupportedProvider(provider) => {
            ModelError::UnsupportedProvider(provider.clone())
        }
        ModelError::InvalidResponse(message) => ModelError::InvalidResponse(message.clone()),
        ModelError::RetryableInvalidResponse {
            error_type,
            message,
        } => ModelError::RetryableInvalidResponse {
            error_type: error_type.clone(),
            message: message.clone(),
        },
        ModelError::ProviderReported {
            kind,
            error_type,
            message,
        } => ModelError::ProviderReported {
            kind: *kind,
            error_type: error_type.clone(),
            message: message.clone(),
        },
        ModelError::Interrupted => ModelError::Interrupted,
        ModelError::StreamIdleTimeout { timeout } => {
            ModelError::StreamIdleTimeout { timeout: *timeout }
        }
        ModelError::StreamFailedAfterOutput { message } => ModelError::StreamFailedAfterOutput {
            message: message.clone(),
        },
        ModelError::HttpStatus { status, body } => ModelError::HttpStatus {
            status: *status,
            body: body.clone(),
        },
        ModelError::Io(_) => ModelError::InvalidResponse("provider I/O failed".into()),
        ModelError::Request(_) => ModelError::InvalidResponse("provider request failed".into()),
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
        Box::pin(async move {
            Err(super::sdk_contract::provider_error_from_model_error(
                clone_model_error(&self.error),
            ))
        })
    }
}

impl fmt::Display for UnavailableProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.error)
    }
}
