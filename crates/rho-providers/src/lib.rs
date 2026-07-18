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
