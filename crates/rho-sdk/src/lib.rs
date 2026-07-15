//! Embeddable, headless agent runtime for Rho.
//!
//! `rho-sdk` is being extracted from the Rho coding agent. Its default feature
//! set is intentionally empty: constructing the SDK does not grant filesystem,
//! process, network, credential-store, or persistence access. Those capabilities
//! will be exposed through explicit adapters and opt-in features as their stable
//! contracts are added.
//!
//! The application crate remains the owner of CLI parsing, terminal behavior,
//! interactive rendering, keybindings, updates, and application configuration.

#![forbid(unsafe_code)]

mod cancellation;
mod error;
mod id;
pub mod model;
pub mod provider;
mod reasoning;
pub mod tool;

pub use cancellation::CancellationToken;
pub use error::{Error, ProviderError, ProviderErrorKind, Retryability};
pub use id::{InvalidId, Revision, RunId, SessionId, ToolCallId};
pub use reasoning::{ParseReasoningLevelError, ReasoningLevel};
