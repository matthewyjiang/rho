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
//!
//! # Completion
//!
//! ```
//! use rho_sdk::{
//!     model::{ContentBlock, ModelIdentity, ModelResponse},
//!     provider::{ScriptedProvider, ScriptedTurn},
//!     Rho, SessionOptions,
//! };
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), rho_sdk::Error> {
//! let provider = ScriptedProvider::new(
//!     ModelIdentity::new("scripted", "test", "model"),
//!     [ScriptedTurn::completed(ModelResponse::Assistant(vec![
//!         ContentBlock::Text("hello".into()),
//!     ]))],
//! );
//! let rho = Rho::builder().provider(provider).build()?;
//! let session = rho.session(SessionOptions::default()).await?;
//! let outcome = session.complete("say hello").await?;
//! assert_eq!(outcome.text(), "hello");
//! # Ok(())
//! # }
//! ```
//!
//! [`Session::start`](crate::Session::start) provides ordered semantic events,
//! bounded backpressure, a cancellation handle, and a typed final outcome. A
//! dropped [`Run`](crate::Run) cancels and aborts its provider or tool future so
//! one session never retains abandoned work.

#![forbid(unsafe_code)]

mod cancellation;
mod client;
mod compaction;
mod diagnostics;
mod error;
mod event;
mod id;
pub mod model;
mod orchestration;
mod persistence;
pub mod provider;
mod reasoning;
mod run;
mod session;
pub mod tool;
mod workspace;

pub use cancellation::CancellationToken;
pub use client::{Rho, RhoBuilder, SessionOptions, SystemPrompt};
pub use compaction::{
    CompactionFuture, CompactionOutcome, CompactionOutput, CompactionPolicy, CompactionRequest,
    CompactionState, CompactionTrigger, Compactor, ScriptedCompactor,
};
pub use diagnostics::{DiagnosticsSnapshot, PromptSource, PromptSourceKind};
pub use error::{Error, ProviderError, ProviderErrorKind, Retryability};
pub use event::{RunEvent, RunOutcome, StopReason, ToolCompletion, ToolFailure};
pub use id::{InvalidId, Revision, RunId, SessionId, ToolCallId};
pub use persistence::{InMemorySessionStore, SessionSnapshot, SESSION_SNAPSHOT_SCHEMA_VERSION};
pub use reasoning::{ParseReasoningLevelError, ReasoningLevel};
pub use run::Run;
pub use session::{Session, SessionState, UserInput};
pub use workspace::{
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CapabilityRequest,
    DenyAllPolicy, DenyApprovals, PolicyDecision, ScopedWorkspacePolicy, Workspace,
    WorkspacePolicy,
};

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod runtime_tests;
