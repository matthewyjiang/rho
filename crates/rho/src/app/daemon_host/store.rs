//! The credential store the daemon keeps provider credentials in:
//! `rho/daemon/status`'s `credential_store`, and `rho/auth/choose_store`
//! (#1662).
//!
//! The daemon may have started where the OS store cannot be reached, as a
//! daemon a client started from an SSH login has no session D-Bus for the
//! Secret Service. It keeps failing closed, as every rho process does: a
//! write to a store it cannot reach is refused with
//! `CredentialStoreUnavailable`, and nothing moves to the file store until a
//! client chooses it. The status says which store, what chose it, and why
//! it cannot be reached, so a client can offer that choice.

use rho_protocol::{
    AuthChooseStoreParams, AuthChooseStoreResponse, CredentialStoreKind, CredentialStoreStatus,
    EnvironmentOverride, ErrorCode, RpcError, StoreSelectedBy,
};
use rho_providers::credentials::{check_credential_store, CredentialError, CredentialStoreBackend};

use super::{
    auth::{self, credentials_changed},
    sessions::Sessions,
};
use crate::credential_store::{self, SelectedBy, StoreSelection};

/// The store the daemon's environment and config select now, and whether
/// the daemon can reach it. `environment` is the daemon's parsed
/// `RHO_CREDENTIAL_STORE`. The check writes nothing, but the OS store's
/// first connection may wait on D-Bus, so it runs off the async workers.
pub(super) async fn status(environment: Option<CredentialStoreBackend>) -> CredentialStoreStatus {
    // A panic here is answered as an internal error, as any handler's is.
    tokio::task::spawn_blocking(move || current(environment))
        .await
        .unwrap_or_else(|error| panic!("the credential store check did not finish: {error}"))
}

/// Keeps provider credentials in `params.store` from now on, once it took a
/// temporary secret, and saves the choice in the daemon's config. Refused
/// when the daemon's environment chooses the store, which wins over config.
pub(super) async fn choose(
    params: AuthChooseStoreParams,
    environment: Option<CredentialStoreBackend>,
    sessions: &Sessions,
) -> Result<AuthChooseStoreResponse, RpcError> {
    if environment.is_some() {
        let variable = credential_store::ENV_BACKEND;
        let data = serde_json::to_value(EnvironmentOverride {
            variable: variable.to_owned(),
        })
        .expect("an environment override serializes");
        return Err(RpcError::new(
            ErrorCode::EnvironmentOverride,
            format!(
                "{variable} in the daemon's environment chooses its credential store; unset it there and restart the daemon to choose another"
            ),
        )
        .with_data(data));
    }
    let backend = match params.store {
        CredentialStoreKind::Os => CredentialStoreBackend::Os,
        CredentialStoreKind::File => CredentialStoreBackend::File,
        _ => {
            return Err(RpcError::new(
                ErrorCode::InvalidParams,
                "unknown credential store; expected os or file",
            ))
        }
    };
    let credential_store = auth::off_workers(move || {
        let probe = credential_store::probe(backend);
        if !probe.available {
            return Err(RpcError::new(
                ErrorCode::CredentialStoreUnavailable,
                format!(
                    "the daemon could not keep a credential in the {} store: {}",
                    backend.as_str(),
                    probe.detail
                ),
            ));
        }
        let internal = |error: anyhow::Error| {
            RpcError::new(
                ErrorCode::InternalError,
                format!("could not save the credential store choice: {error:#}"),
            )
        };
        let path = auth::repository().configured_path().map_err(internal)?;
        credential_store::save_backend(backend, Some(path)).map_err(internal)?;
        Ok(current(environment))
    })
    .await??;
    tracing::info!(store = backend.as_str(), "chose the credential store");
    // The models each session may switch to follow the store.
    credentials_changed(sessions).await;
    Ok(AuthChooseStoreResponse { credential_store })
}

fn current(environment: Option<CredentialStoreBackend>) -> CredentialStoreStatus {
    let StoreSelection {
        backend,
        selected_by,
    } = credential_store::current_selection(environment);
    CredentialStoreStatus {
        store: match backend {
            CredentialStoreBackend::Os => CredentialStoreKind::Os,
            CredentialStoreBackend::File => CredentialStoreKind::File,
        },
        selected_by: match selected_by {
            SelectedBy::Environment => StoreSelectedBy::Environment,
            SelectedBy::Config => StoreSelectedBy::Config,
            SelectedBy::Default => StoreSelectedBy::Default,
        },
        // Only why: the error's generic advice on choosing a store is the
        // client's to give.
        unreachable: check_credential_store(backend)
            .err()
            .map(|error| match error {
                CredentialError::StoreUnavailable(reason) => reason,
                error @ CredentialError::InvalidData(_) => error.to_string(),
            }),
    }
}
