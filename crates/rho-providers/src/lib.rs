//! Model provider runtimes, credential storage, and the model catalog.
//!
//! This crate owns everything needed to authenticate against a model
//! provider and stream model responses over the provider's wire protocol:
//!
//! - [`providers`] builds [`rho_sdk::provider::ModelProvider`] instances,
//!   bootstrapping credentials from the environment and the OS keyring.
//! - [`credentials`] and [`auth`] store API keys and OAuth tokens and run
//!   provider login flows.
//! - [`model`] is the canonical request/response contract plus the model
//!   catalog and metadata caches.
//! - [`protocol`] translates between the canonical contract and provider
//!   wire formats.

use std::sync::OnceLock;

static RHO_VERSION: OnceLock<&'static str> = OnceLock::new();

/// Configures the application version used in provider request headers.
///
/// Embedders should call this once, before creating providers. If no version is
/// configured, request headers use this crate's package version.
pub fn set_rho_version(version: &'static str) -> Result<(), &'static str> {
    RHO_VERSION.set(version).map_err(|_| rho_version())
}

/// Returns the application version used in provider request headers.
pub fn rho_version() -> &'static str {
    RHO_VERSION.get_or_init(|| env!("CARGO_PKG_VERSION"))
}

pub(crate) fn rho_user_agent() -> String {
    format!("rho/{}", rho_version())
}

pub mod auth;
pub mod credentials;
pub mod model;
pub mod paths;
pub mod protocol;
pub mod provider;
pub mod provider_backend;
pub mod providers;
pub mod reasoning;

pub use credentials::{CredentialError, CredentialResult, CredentialStore, OsCredentialStore};
pub use model::ModelError;
pub use providers::{
    build_automation_provider, build_sdk_provider, build_sdk_provider_with_source,
    ProviderBuildOptions, UnavailableProvider,
};
pub use rho_tools as tools;
