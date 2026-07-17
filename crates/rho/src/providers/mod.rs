//! Provider runtimes own authentication, endpoints, retries, and transport policy.
//!
//! Wire-format conversion belongs in [`crate::protocol`]. Providers may share a
//! protocol while retaining different authentication and runtime behavior.
//!
//! Built-in providers implement [`rho_sdk::provider::ModelProvider`] directly.
//! [`sdk_contract`] only holds shared error sanitization and callback-stream
//! forwarding helpers.

pub(crate) mod anthropic;
#[cfg(debug_assertions)]
mod automation_fixture;
pub(crate) mod builder;
mod factory;
pub(crate) mod github_copilot;
pub(crate) mod openai;
pub(crate) mod openai_compatible;
pub(crate) mod sdk_contract;
mod send_stream;
#[cfg(debug_assertions)]
mod tui_fixture;
pub(crate) mod xai;

pub(crate) use builder::ProviderBuildOptions;
pub(crate) use factory::{
    build_automation_provider, build_sdk_provider, build_sdk_provider_with_source,
    UnavailableProvider,
};
