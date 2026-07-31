//! Typed, bounded lifecycle hooks.
//!
//! Hooks let trusted host automation observe what an agent does and, for one
//! pre-action event, deny an operation. They are an enforcement and observation
//! layer, not a workflow engine and not a permission grant.
//!
//! # Split of responsibilities
//!
//! This module owns the generic machinery: the [`HookEventKind`] vocabulary, the
//! [`HookEnvelope`] wire contract, [`HookDecision`], payload bounds, and the two
//! extension points a host implements. It owns no configuration format, no
//! process spawning, and no trust policy; those belong to the host, which knows
//! where hook programs come from and what makes one trustworthy.
//!
//! # Extension points
//!
//! - [`PreToolUseGate`] is consulted inside the existing authorization path,
//!   after [`WorkspacePolicy::evaluate`](crate::WorkspacePolicy::evaluate) and
//!   before any approval await. It may only keep the current decision or make it
//!   stricter.
//! - [`HookObserver`] receives every delivered observational event. It must
//!   enqueue and return rather than doing work inline.
//!
//! `before_tool_use` is a question, not a notification: it reaches the gate once
//! and is not repeated to the observer. Every other delivered event reaches only
//! the observer.
//!
//! # Composition with host policy
//!
//! | Host policy       | Hook result | Outcome                  |
//! | ----------------- | ----------- | ------------------------ |
//! | `Deny`            | not called  | deny (policy)            |
//! | `RequireApproval` | `Continue`  | approval still required  |
//! | `RequireApproval` | `Deny`      | deny before the prompt   |
//! | `Allow`           | `Continue`  | execute                  |
//! | `Allow`           | `Deny`      | deny                     |
//!
//! # Payload safety
//!
//! Envelopes carry structured capability facts built from the request the host
//! policy already saw, not scraped argument prose. Paths and shell command text
//! are included because a deny hook exists to inspect them. Credentials,
//! authorization headers, environment values, and URL query strings are not.
//! Every envelope reports what was shortened in [`HookTruncation`].

mod bounds;
mod dispatch;
mod envelope;
mod event;
mod gate;
mod payload;
pub mod testing;

pub(crate) use dispatch::HookWiring;
pub(crate) use payload::{bounded_failure, error_label, summarize_capability, BoundedFailure};

pub use bounds::{
    HookPayloadBounds, HookTruncation, DEFAULT_MAX_ENVELOPE_BYTES, DEFAULT_MAX_FIELD_BYTES,
};
pub use dispatch::{
    HookDelegation, HookDispatcher, HookObserveFuture, HookObserver, HookSessionFailureKind,
};
pub use envelope::{
    HookEnvelope, HookEnvelopeError, HookEnvelopeTooLarge, HookIdentity, HOOK_SCHEMA_VERSION,
};
pub use event::HookEventKind;
pub use gate::{AllowAllGate, HookDecision, HookGateFuture, PreToolUseGate, PreToolUseRequest};
pub use payload::{
    AfterToolUsePayload, BeforeToolUsePayload, HookCapability, HookFailure, HookPathScope,
    HookPayload, HookPolicyOutcome, HookProcessEnvironment, HookStopReason, HookTool,
    HookToolStatus, HookWorkspace, RunCompletedPayload, RunFailedPayload, SessionCompletedPayload,
    SessionFailedPayload, SessionStartedPayload, PROMPT_CONSTRUCTION_TOOL,
};
