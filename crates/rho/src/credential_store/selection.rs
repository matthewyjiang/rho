//! Which credential store a process uses, and what chose it.

use rho_providers::credentials::{CredentialResult, CredentialStoreBackend};

/// What chose a process's credential store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectedBy {
    /// `RHO_CREDENTIAL_STORE` in the process's environment.
    Environment,
    /// `behavior.credential_store` in config.toml.
    Config,
    /// Neither, so the OS store. A login asks to choose one explicitly.
    Default,
}

/// The credential store a process uses and what chose it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StoreSelection {
    pub(crate) backend: CredentialStoreBackend,
    pub(crate) selected_by: SelectedBy,
}

impl StoreSelection {
    /// `environment`, a parsed `RHO_CREDENTIAL_STORE`, wins over `config`;
    /// with neither, the OS store. Never the file store unless chosen.
    pub(crate) fn resolve(
        environment: Option<CredentialStoreBackend>,
        config: Option<CredentialStoreBackend>,
    ) -> Self {
        match (environment, config) {
            (Some(backend), _) => Self {
                backend,
                selected_by: SelectedBy::Environment,
            },
            (None, Some(backend)) => Self {
                backend,
                selected_by: SelectedBy::Config,
            },
            (None, None) => Self {
                backend: CredentialStoreBackend::Os,
                selected_by: SelectedBy::Default,
            },
        }
    }
}

/// The backend a `RHO_CREDENTIAL_STORE` of `value` names; `None` when it is
/// unset or blank. An unknown value is an error, never a fallback.
pub(crate) fn parse_environment(
    value: Option<&str>,
) -> CredentialResult<Option<CredentialStoreBackend>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(CredentialStoreBackend::parse)
        .transpose()
}
