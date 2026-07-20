//! Provider runtimes own authentication, endpoints, retries, and transport policy.
//!
//! Wire-format conversion belongs in [`crate::protocol`]. Providers may share a
//! protocol while retaining different authentication and runtime behavior.
//!
//! Built-in providers implement [`rho_sdk::provider::ModelProvider`] directly.
//! [`sdk_contract`] only holds shared error sanitization and callback-stream
//! forwarding helpers.

pub mod anthropic;
#[cfg(debug_assertions)]
mod automation_fixture;
pub mod builder;
pub mod factory;
pub mod github_copilot;
pub mod google;
pub mod openai;
pub mod openai_compatible;
pub mod sdk_contract;
pub mod send_stream;
#[cfg(debug_assertions)]
mod tui_fixture;
pub mod xai;

pub use builder::ProviderBuildOptions;
pub use factory::{
    build_automation_provider, build_sdk_provider, build_sdk_provider_with_source,
    UnavailableProvider,
};
