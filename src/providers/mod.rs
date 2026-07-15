//! Provider runtimes own authentication, endpoints, retries, and transport policy.
//!
//! Wire-format conversion belongs in [`crate::protocol`]. Providers may share a
//! protocol while retaining different authentication and runtime behavior.
//!
//! Public SDK consumption goes through [`sdk_adapter`], which adapts these
//! transports to [`rho_sdk::provider::ModelProvider`] without duplicating
//! transport logic.

pub(crate) mod anthropic;
#[cfg(debug_assertions)]
mod automation_fixture;
mod factory;
pub(crate) mod github_copilot;
pub(crate) mod openai;
pub(crate) mod sdk_adapter;
mod send_stream;
pub(crate) mod xai;

pub(crate) use factory::{build_automation_provider, build_provider, UnavailableProvider};
