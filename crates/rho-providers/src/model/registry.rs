use crate::{model::ModelError, provider};

pub use crate::provider::ProviderRuntime;

/// Runtime construction data for a registered provider name.
///
/// Projection of [`provider::ProviderDescriptor::runtime`] so callers keep a
/// stable registry entry point without parallel match arms.
pub fn provider_runtime(provider: &str) -> Option<ProviderRuntime> {
    Some(
        provider::resolve_provider_reference(provider)
            .ok()?
            .provider
            .runtime,
    )
}

pub fn missing_credential_error(message: &'static str) -> ModelError {
    ModelError::missing_credentials(message)
}

pub fn missing_credentials_error(provider_name: &str) -> ModelError {
    let selected = provider::resolve_auth_mode(provider_name)
        .map(|(_, mode)| mode)
        .or_else(|| {
            provider::legacy_provider_alias(provider_name).and_then(|(provider, auth)| {
                provider::provider_descriptor(provider)?.auth_mode(auth)
            })
        });
    if let Some(mode) = selected {
        return mode.auth_kind.missing_message().map_or_else(
            || {
                ModelError::InvalidResponse(format!(
                    "provider '{provider_name}' does not require credentials"
                ))
            },
            ModelError::missing_credentials,
        );
    }
    match provider::provider_descriptor(provider_name) {
        Some(descriptor) if descriptor.is_keyless() => ModelError::InvalidResponse(format!(
            "provider '{provider_name}' does not require credentials"
        )),
        Some(descriptor) => match descriptor.default_auth().auth_kind.missing_message() {
            Some(message) => ModelError::missing_credentials(message),
            None => ModelError::InvalidResponse(format!(
                "provider '{provider_name}' does not require credentials"
            )),
        },
        None => ModelError::UnsupportedProvider(provider_name.to_string()),
    }
}
