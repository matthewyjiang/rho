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
//! [`Client`] implements the frontend side; [`serve_connection`] implements
//! the daemon side around a [`Service`].
//!
//! # Versioning
//!
//! [`PROTOCOL_VERSION`] is independent of this crate's version. A protocol
//! minor only adds optional fields, methods, capabilities, and error codes;
//! receivers ignore fields they do not know.

mod client;
mod error;
mod jsonrpc;
pub mod methods;
mod server;
mod version;

pub use client::{Client, ClientError};
pub use error::ErrorCode;
pub use jsonrpc::{
    FrameError, Message, MessageReader, MessageWriter, Notification, ParseError, Request,
    RequestId, Response, RpcError, MAX_LINE_BYTES,
};
pub use methods::{
    ClientInfo, DaemonInfo, DaemonStatus, HelloParams, HelloResponse, StatusParams, StopParams,
    StopResponse,
};
pub use server::{serve_connection, ServerInfo, Service};
pub use version::{ProtocolVersion, VersionFix, VersionMismatch, PROTOCOL_VERSION};
