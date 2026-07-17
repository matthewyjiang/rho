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
//! # Runtime ownership
//!
//! - [`Rho`] is the process-scoped runtime built from explicit providers, tools,
//!   policies, and optional adapters.
//! - [`Session`] owns one conversation history and permits a single active
//!   [`Run`].
//! - [`Run`] exposes ordered [`RunEvent`]s, cooperative cancellation, host input
//!   responses, and a typed [`RunOutcome`].
//!
//! Construction is side-effect-free by default: no automatic writes to `~/.rho`,
//! no implicit environment reads, no credential-store access, and no terminal or
//! logger setup.
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
//! # Streaming
//!
//! [`Session::start`](crate::Session::start) provides ordered semantic events,
//! bounded backpressure, a cancellation handle, and a typed final outcome. A
//! dropped [`Run`](crate::Run) cancels and aborts its provider or tool future so
//! one session never retains abandoned work.
//!
//! ```
//! use rho_sdk::{
//!     model::{ContentBlock, ModelEvent, ModelIdentity, ModelResponse},
//!     provider::{ScriptedProvider, ScriptedTurn},
//!     Rho, RunEvent, SessionOptions, UserInput,
//! };
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), rho_sdk::Error> {
//! let provider = ScriptedProvider::new(
//!     ModelIdentity::new("scripted", "test", "model"),
//!     [ScriptedTurn::streaming(
//!         vec![ModelEvent::OutputDelta("hi".into())],
//!         ModelResponse::Assistant(vec![ContentBlock::Text("hi".into())]),
//!     )],
//! );
//! let rho = Rho::builder().provider(provider).build()?;
//! let session = rho.session(SessionOptions::default()).await?;
//! let mut run = session.start(UserInput::text("stream")).await?;
//! while let Some(event) = run.next_event().await {
//!     if let RunEvent::AssistantTextDelta { text } = event {
//!         assert_eq!(text, "hi");
//!     }
//! }
//! assert_eq!(run.outcome().await?.text(), "hi");
//! # Ok(())
//! # }
//! ```
//!
//! # Extension points
//!
//! Hosts can supply custom [`ModelProvider`](crate::provider::ModelProvider)
//! and [`Tool`](crate::tool::Tool) implementations. Both return explicit `Send`
//! futures suitable for trait objects. Tools use only capabilities supplied
//! through [`ToolContext`](crate::tool::ToolContext).
//!
//! # Session snapshots
//!
//! [`Session::snapshot`](crate::Session::snapshot) produces a versioned,
//! JSON-serializable [`SessionSnapshot`] restored through
//! [`SessionOptions::from_snapshot`](crate::SessionOptions::from_snapshot).
//! [`InMemorySessionStore`] is a concrete atomic adapter for tests and simple
//! hosts. Snapshots never include credentials or raw reasoning.
//!
//! # Cancellation and host interaction
//!
//! - Cancel a run with [`Run::cancel`](crate::Run::cancel) or
//!   [`Run::cancellation_handle`](crate::Run::cancellation_handle).
//! - Retract staged steering with [`Run::retract_steering`] before it is
//!   appended to conversation history.
//! - Answer questionnaires with [`Run::respond`](crate::Run::respond) after
//!   [`RunEvent::HostInputRequested`].
//! - Gate sensitive work with [`WorkspacePolicy`] and [`ApprovalHandler`].
//!
//! The compiling examples under `examples/` cover simple completion, streaming,
//! custom providers, custom tools, snapshot restore, image/history input,
//! cancellation, and questionnaire/approval flows.
//!
//! # Security defaults
//!
//! The default feature set is empty. Creating an SDK runtime does not
//! implicitly read environment variables, access an OS credential store, write
//! to `~/.rho`, initialize a terminal or logger, check for updates, or grant
//! tools filesystem, process, or network access.

#![forbid(unsafe_code)]

mod cancellation;
mod client;
mod compaction;
mod diagnostics;
mod error;
mod event;
mod host_input;
mod id;
pub mod model;
mod orchestration;
mod persistence;
pub mod provider;
mod reasoning;
mod run;
mod secret;
mod session;
mod steering;
pub mod tool;
mod usage;
mod workspace;

pub use cancellation::CancellationToken;
pub use client::{Rho, RhoBuilder, SessionOptions, ShutdownOutcome, SystemPrompt};
pub use compaction::{
    CompactionFuture, CompactionOutcome, CompactionOutput, CompactionPolicy, CompactionRequest,
    CompactionState, CompactionThreshold, CompactionTrigger, Compactor, CompactorCancellationMode,
    ScriptedCompactor,
};
pub use diagnostics::{DiagnosticsSnapshot, PromptSource, PromptSourceKind, ToolDiagnostic};
pub use error::{Error, ProviderDiagnostic, ProviderError, ProviderErrorKind, Retryability};
pub use event::{
    RunEvent, RunOutcome, StopReason, ToolCompletion, ToolFailure,
    PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY, PROVIDER_ACTIVITY_WEB_SEARCH,
};
pub use host_input::{
    HostChoice, HostInputRequest, HostInputResponse, HostQuestion, SelectionMode,
};
pub use id::{HostInputId, InvalidId, Revision, RunId, SessionId, SteeringId, ToolCallId};
pub use persistence::{
    InMemorySessionStore, SessionSnapshot, SessionStore, SessionStoreFuture,
    MIN_SESSION_SNAPSHOT_SCHEMA_VERSION, SESSION_SNAPSHOT_SCHEMA_VERSION,
};
pub use reasoning::{ParseReasoningLevelError, ReasoningLevel};
pub use run::Run;
pub use secret::SecretString;
pub use session::{Session, SessionState, UserInput};
pub use steering::SteeringRetraction;
pub use usage::{
    ProviderRequestOutcome, ProviderRequestUsageContext, ProviderRequestUsageEvent,
    ProviderRequestUsageRecorder, ProviderRequestUsageRecorderError,
    ProviderRequestUsageRecorderFuture, UsageRecorderDiagnostic,
};
pub use workspace::{
    approval_channel, ApprovalAuditDecision, ApprovalAuditRecord, ApprovalDecision, ApprovalFuture,
    ApprovalHandler, ApprovalRequest, ApprovalRequestReceiver, AuthorizationDenialKind,
    AuthorizationError, AuthorizationOutcome, CapabilityKind, CapabilityOperation,
    CapabilityRequest, CapabilitySource, ChannelApprovalHandler, DenyAllPolicy, DenyApprovals,
    ExecutableSelection, NetworkTarget, PathScope, PendingApproval, PolicyDecision,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits,
    ResolvedWorkspacePath, ScopedWorkspacePolicy, Workspace, WorkspacePathError,
    WorkspacePathErrorKind, WorkspacePathState, WorkspacePolicy,
};

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod runtime_tests;
