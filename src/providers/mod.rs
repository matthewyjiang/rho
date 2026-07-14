//! Provider runtimes own authentication, endpoints, retries, and transport policy.
//!
//! Wire-format conversion belongs in [`crate::protocol`]. Providers may share a
//! protocol while retaining different authentication and runtime behavior.

pub(crate) mod anthropic;
mod factory;
pub(crate) mod github_copilot;
pub(crate) mod openai;
pub(crate) mod xai;

pub(crate) use factory::{build_provider, UnavailableProvider};
