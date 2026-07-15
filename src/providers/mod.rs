//! Provider runtimes own authentication, endpoints, retries, and transport policy.
//!
//! Wire-format conversion belongs in [`crate::protocol`]. Providers may share a
//! protocol while retaining different authentication and runtime behavior.
//!
//! Public SDK consumption goes through [`sdk_adapter`], which adapts these
//! transports to [`rho_sdk::provider::ModelProvider`] without duplicating
//! transport logic.

pub(crate) mod anthropic;
mod factory;
pub(crate) mod github_copilot;
pub(crate) mod openai;
pub(crate) mod sdk_adapter;
mod send_stream;
pub(crate) mod xai;

pub(crate) use factory::{build_provider, UnavailableProvider};
// SDK migration surface: reachable for embedders and covered by adapter tests.
#[allow(unused_imports)]
pub(crate) use factory::build_sdk_provider;
#[allow(unused_imports)]
pub(crate) use sdk_adapter::{
    provider_error_from_model_error, AdaptableProvider, SdkProviderAdapter,
};
