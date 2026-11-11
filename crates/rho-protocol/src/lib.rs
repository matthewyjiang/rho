//! The protocol between the Rho daemon and its frontends.
//!
//! Frontends (the Rho TUI, rho-ui, the `rho acp` adapter, MCP) talk to the
//! per-user Rho daemon over newline-delimited JSON-RPC 2.0. This crate owns
//! that wire contract and has no dependency on `rho-sdk`, so SDK majors never
//! break a frontend build.
//!
//! # Connection lifecycle
//!
//! 1. The client sends [`methods::HELLO`] with its [`PROTOCOL_VERSION`].
//!    Any other request first gets [`ErrorCode::HandshakeRequired`].
//! 2. The daemon accepts a compatible version
//!    ([`ProtocolVersion::is_compatible_with`]), or replies
//!    [`ErrorCode::VersionMismatch`] with a [`VersionMismatch`] saying which
//!    side to change and, for a client newer than the daemon, what
//!    restarting the daemon would cost ([`RestartCost`]).
//! 3. The client sends requests such as [`methods::DAEMON_STATUS`]. The
//!    daemon serves them concurrently and answers each one when it completes,
//!    so responses can arrive out of order; each carries its request's id.
//!    Requests also run concurrently: send one that depends on another only
//!    after that one's response arrives.
//! 4. When the client stops sending or the daemon shuts down, the daemon
//!    answers every request it has already read, then closes the connection.
//!
//! # Events
//!
//! A client follows a session by attaching to it ([`methods::SESSION_ATTACH`]).
//! The daemon then sends the session's events as
//! [`methods::SESSION_EVENT`] notifications, each numbered with its
//! [`Seq`] in the session's [`StreamId`]. A client that reattaches with its
//! [`Cursor`] resumes after the last event it saw, or is reset to the
//! session's saved transcript when the daemon no longer buffers that point
//! (#1657). A client that only reads a session observes it instead
//! ([`methods::SESSION_OBSERVE`]), which follows a loaded session the same
//! way but never loads one. A tool call's events carry the card Rho's TUI draws for it, in
//! the types of [`display`].
//!
//! A client signs the daemon in with a provider's interactive login through
//! [`methods::AUTH_LOGIN`]: the login runs in the daemon, which relays its
//! prompt and end to that client as notifications on the connection that
//! started it, and cancels it when that connection closes (#1662).
//!
//! [`Client`] implements the frontend side, with daemon notifications on its
//! [`Notifications`]; [`serve_connection`] implements the daemon side around a
//! [`Service`], which publishes events through [`subscription()`].
//!
//! # Methods
//!
//! [`methods`] lists every method with its params and result types. A
//! session's turn runs in the daemon and outlives the connection that
//! started it; a client that reconnects follows it through
//! [`methods::JOB_LIST`].
//!
//! # Versioning
//!
//! [`PROTOCOL_VERSION`] is independent of this crate's version. While its
//! major is 0 the protocol is unstable: every change to the wire raises the
//! minor, and a client and daemon work together only when both numbers
//! match. From major 1 on, a protocol minor only adds optional fields,
//! methods, notifications, capabilities, error codes, and values of enums
//! that read unknown values as `Unknown`; receivers ignore fields they do
//! not know. [`schema::document`] is the protocol as JSON Schema, committed
//! as `schema/protocol.json`, and CI holds the protocol to these rules: each
//! change while the major is 0, and each release against the last one's
//! from major 1 on.

mod client;
mod declared;
pub mod display;
mod error;
mod forward;
mod jsonrpc;
mod login_relay;
pub mod methods;
pub mod schema;
mod server;
mod server_calls;
mod server_settings;
mod server_streams;
mod subscription;
mod version;

pub use client::{Client, ClientError, DaemonNotification, Notifications};
pub use error::ErrorCode;
pub use forward::ConfigWatch;
pub use jsonrpc::{
    FrameError, Message, MessageReader, MessageWriter, Notification, ParseError, Request,
    RequestId, Response, RpcError, MAX_LINE_BYTES,
};
pub use login_relay::{login_relay, LoginRelay, LoginStream};
pub use methods::{
    AnswerStop, ApprovalAsk, ApprovalSource, Ask, AuthKeyParams, AuthKeyResponse, AuthLogoutParams,
    AuthLogoutResponse, Budget, CardTone, Choice, ClientCapability, ClientEnvironment, ClientHello,
    ClientInfo, ClientRole, CompactionCounts, CompactionId, CompactionStatus, CompactionTrigger,
    ContentBlock, Cursor, DaemonInfo, DaemonStatus, DefaultAnswer, DefaultStyle, DetachReason,
    EnvironmentOverride, GrantAccess, GrantWorkspaces, HelloParams, HelloResponse, IdempotencyKey,
    InputAnswer, InputAsker, InputRequest, InputRequestId, InputTimeout, JobId, JobKind,
    JobListParams, JobListResponse, JobState, JobSummary, Location, MessageId, Operation,
    PathScope, PendingUpgrade, PermissionMode, PlanApprovalAsk, PlanEntry, PlanStatus,
    ProjectTrustMismatch, PromptAuthor, Question, QuestionAsk, QueuePlace, Replay, Resolution,
    RestartParams, RestartResponse, RetryReason, Selection, Seq, SessionAnswerParams,
    SessionAnswerResponse, SessionAttachParams, SessionAttachResponse, SessionCancelParams,
    SessionCancelResponse, SessionControlAction, SessionDetachParams, SessionDetachResponse,
    SessionDetachedParams, SessionEvent, SessionEventParams, SessionGoal, SessionGoalAction,
    SessionGoalPhase, SessionGoalStep, SessionGrant, SessionHoldParams, SessionHoldResponse,
    SessionId, SessionNewParams, SessionNewResponse, SessionObserveParams, SessionPromptParams,
    SessionPromptResponse, SessionSteerParams, SessionSteerResponse, SessionUnsteerParams,
    SessionUnsteerResponse, StatusParams, SteerId, StopParams, StopResponse, StreamId, ToolCallId,
    ToolCallUpdate, ToolContent, ToolKind, ToolStatus, TurnError, TurnErrorKind, TurnId,
    TurnOutcome, UncheckedKey, UncheckedReason, UnsteerOutcome, SESSION_ANSWER, SESSION_HOLD,
};
pub use methods::{
    Applies, ClientName, ConfigCategory, ConfigChangedParams, ConfigError, ConfigGetParams,
    ConfigGetResponse, ConfigLayer, ConfigNamespace, ConfigRegisterParams, ConfigRegisterResponse,
    ConfigRevision, ConfigSchema, ConfigSchemaParams, ConfigSetParams, ConfigSetResponse,
    ConfigWriter, FileOnlyTable, IntegerUnit, InvalidClientName, ModelRuntime, ModelSelection,
    SettingChange, SettingChoice, SettingEntry, SettingKey, SettingKind, SettingSpec, SettingValue,
    ValueSource,
};
pub use methods::{
    AuthChooseStoreParams, AuthChooseStoreResponse, CredentialStoreKind, CredentialStoreStatus,
    StoreSelectedBy,
};
pub use methods::{
    AuthKind, AuthListParams, AuthListResponse, AuthLoginCancelParams, AuthLoginCancelResponse,
    AuthLoginFinishedParams, AuthLoginParams, AuthLoginPromptParams, AuthLoginResponse, AuthStatus,
    AuthSummary, LoginFlow, LoginId, LoginOutcome, LoginPrompt,
};
pub use methods::{
    ChildThreads, IdleWake, McpServer, SecretMap, SessionPart, SessionSetup, SignIn,
};
pub use methods::{
    CommandKind, FastModeOptions, ModelChoice, ModelOptions, ModelRef, PermissionModeOptions,
    ReasoningLevel, ReasoningOptions, SessionChange, SessionCommand, SessionConfigureParams,
    SessionConfigureResponse, SessionOptions, SESSION_CONFIGURE,
};
pub use methods::{
    ProcessChunk, ProcessOutput, ProcessOutputParams, ProcessState, ProcessStream, SessionProcess,
    SessionProcessesParams, SessionProcessesResponse, SESSION_PROCESSES,
};
pub use methods::{
    SessionDeleteParams, SessionDeleteResponse, SessionListParams, SessionListResponse,
    SessionRenameParams, SessionRenameResponse, SessionSummary, SESSION_DELETE, SESSION_LIST,
    SESSION_RENAME,
};
pub use methods::{
    SessionInspectParams, SessionInspectResponse, SessionMcpInventory, SessionMcpMode,
    SessionMcpParams, SessionMcpServer, SessionMcpStatus, SessionReport, SESSION_INSPECT,
    SESSION_MCP,
};
pub use methods::{
    SessionMcpCatalog, SessionMcpCatalogParams, SessionMcpCompleteParams,
    SessionMcpCompleteResponse, SessionMcpPrompt, SessionMcpPromptArgument, SessionMcpResource,
    SessionMcpResourceContent, SessionMcpResourceParams, SessionMcpResourceResponse,
    SESSION_MCP_CATALOG, SESSION_MCP_COMPLETE, SESSION_MCP_RESOURCE,
};
pub use server::{serve_connection, ServerInfo, Service, CLOSE_GRACE};
pub use server_settings::SettingsService;
pub use subscription::{subscription, Published, Publisher, Subscription};
pub use version::{ProtocolVersion, RestartCost, VersionFix, VersionMismatch, PROTOCOL_VERSION};
