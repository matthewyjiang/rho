//! The credential store as `rho-protocol` names it, for the daemon and its
//! clients (#1662).

use rho_protocol::{CredentialStoreKind, CredentialStoreStatus, StoreSelectedBy};
use rho_providers::credentials::CredentialStoreBackend;

use super::{SelectedBy, StoreSelection};

/// `backend` on the wire.
pub(crate) fn kind(backend: CredentialStoreBackend) -> CredentialStoreKind {
    match backend {
        CredentialStoreBackend::Os => CredentialStoreKind::Os,
        CredentialStoreBackend::File => CredentialStoreKind::File,
    }
}

/// The backend `kind` names; `None` for a store this build does not know.
pub(crate) fn backend(kind: CredentialStoreKind) -> Option<CredentialStoreBackend> {
    match kind {
        CredentialStoreKind::Os => Some(CredentialStoreBackend::Os),
        CredentialStoreKind::File => Some(CredentialStoreBackend::File),
        _ => None,
    }
}

/// `selection` on the wire, with why the store cannot be reached, if not.
pub(crate) fn status(
    selection: StoreSelection,
    unreachable: Option<String>,
) -> CredentialStoreStatus {
    CredentialStoreStatus {
        store: kind(selection.backend),
        selected_by: match selection.selected_by {
            SelectedBy::Environment => StoreSelectedBy::Environment,
            SelectedBy::Config => StoreSelectedBy::Config,
            SelectedBy::Default => StoreSelectedBy::Default,
        },
        unreachable,
    }
}

/// How `status`'s store and what chose it read in a sentence, as in
/// "os store, chosen by default".
pub(crate) fn words(status: &CredentialStoreStatus) -> (&'static str, &'static str) {
    let name = backend(status.store).map_or("unknown", CredentialStoreBackend::as_str);
    let chosen = match status.selected_by {
        StoreSelectedBy::Default => "by default",
        StoreSelectedBy::Config => "in config",
        StoreSelectedBy::Environment => "by RHO_CREDENTIAL_STORE",
        _ => "by an unknown setting",
    };
    (name, chosen)
}
