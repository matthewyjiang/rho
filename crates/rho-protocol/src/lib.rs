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
//! 2. The daemon accepts when the majors match, or replies
//!    [`ErrorCode::VersionMismatch`] with a [`VersionMismatch`] saying which
//!    side to change.
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
//! (#1657).
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
//! [`PROTOCOL_VERSION`] is independent of this crate's version. A protocol
//! minor only adds optional fields, methods, capabilities, and error codes;
//! receivers ignore fields they do not know.

mod client;
mod error;
mod forward;
mod jsonrpc;
pub mod methods;
mod server;
mod subscription;
mod version;

pub use client::{Client, ClientError, DaemonNotification, Notifications};
pub use error::ErrorCode;
pub use jsonrpc::{
    FrameError, Message, MessageReader, MessageWriter, Notification, ParseError, Request,
    RequestId, Response, RpcError, MAX_LINE_BYTES,
};
pub use methods::{
    ClientInfo, ContentBlock, Cursor, DaemonInfo, DaemonStatus, DetachReason, HelloParams,
    HelloResponse, JobId, JobKind, JobListParams, JobListResponse, JobState, JobSummary, Location,
    MessageId, PlanEntry, PlanStatus, Replay, Seq, SessionAttachParams, SessionAttachResponse,
    SessionCancelParams, SessionCancelResponse, SessionDetachParams, SessionDetachResponse,
    SessionDetachedParams, SessionEvent, SessionEventParams, SessionId, SessionNewParams,
    SessionNewResponse, SessionPromptParams, SessionPromptResponse, SessionSteerParams,
    SessionSteerResponse, StatusParams, StopParams, StopResponse, StreamId, ToolCallId,
    ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
pub use server::{serve_connection, ServerInfo, Service, CLOSE_GRACE};
pub use subscription::{subscription, Published, Publisher, Subscription};
pub use version::{ProtocolVersion, VersionFix, VersionMismatch, PROTOCOL_VERSION};
