//! The credential store the daemon keeps provider credentials in, and
//! choosing it (#1662).
//!
//! A daemon may have started without what the OS store needs, such as a
//! session D-Bus for the Secret Service when a client started it from an
//! SSH login. It never moves credentials to another store on its own:
//! [`crate::methods::DAEMON_STATUS`] says which store it uses, what chose
//! it, and whether it can reach it, and a client offers the choice and
//! sends it with [`AUTH_CHOOSE_STORE`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Choose the credential store the daemon keeps provider credentials in.
pub const AUTH_CHOOSE_STORE: &str = "rho/auth/choose_store";

/// A store the daemon can keep provider credentials in. Protocol minors may
/// add stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CredentialStoreKind {
    /// The OS keyring: the Secret Service on Linux, the Keychain on macOS.
    Os,
    /// A private file under the daemon's rho home, not encrypted at rest.
    File,
    /// A store a newer peer sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// What chose the daemon's credential store. Protocol minors may add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StoreSelectedBy {
    /// Nothing did, so the daemon uses the OS store. A client signing in
    /// should ask its user to choose one, which [`AUTH_CHOOSE_STORE`] saves.
    Default,
    /// `behavior.credential_store` in the daemon's config.
    Config,
    /// `RHO_CREDENTIAL_STORE` in the daemon's environment, which wins over
    /// the config, so [`AUTH_CHOOSE_STORE`] refuses.
    Environment,
    /// A value a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The credential store the daemon's environment and config select, which
/// every sign-in through the daemon writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialStoreStatus {
    pub store: CredentialStoreKind,
    pub selected_by: StoreSelectedBy,
    /// Why the daemon cannot reach the store, absent when it can. The
    /// daemon checks without writing, so a locked keyring counts as
    /// reachable until a write is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable: Option<String>,
}

/// Params of [`AUTH_CHOOSE_STORE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthChooseStoreParams {
    pub store: CredentialStoreKind,
}

/// Result of [`AUTH_CHOOSE_STORE`]: the daemon wrote and deleted a
/// temporary secret in the store, saved it as `behavior.credential_store`
/// in its config, and keeps credentials there from now on. Credentials
/// already saved stay where they are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthChooseStoreResponse {
    pub credential_store: CredentialStoreStatus,
}
