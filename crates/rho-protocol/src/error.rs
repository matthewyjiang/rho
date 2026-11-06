//! Stable JSON-RPC error codes.

/// Every error code the daemon sends. Codes are part of the wire contract and
/// never change meaning; protocol minors may add codes, so the enum is
/// non-exhaustive and [`crate::RpcError`] keeps the raw integer.
///
/// The first five are the JSON-RPC 2.0 reserved codes. Rho-specific codes use
/// the implementation-defined server range from -32000 down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// -32700: the line was not valid JSON. Sent with a `null` id.
    ParseError,
    /// -32600: the line was JSON but not a valid JSON-RPC 2.0 request, or the
    /// request is not allowed in the connection's current state (a second
    /// `rho/hello`, or a line over [`crate::MAX_LINE_BYTES`]).
    InvalidRequest,
    /// -32601: the daemon does not implement the method.
    MethodNotFound,
    /// -32602: the method exists but its params do not match its schema.
    InvalidParams,
    /// -32603: the daemon failed while handling a valid request.
    InternalError,
    /// -32000: a request other than `rho/hello` arrived before the handshake
    /// completed. Send `rho/hello` first.
    HandshakeRequired,
    /// -32001: `rho/hello` named an incompatible protocol major. `data` is a
    /// [`crate::VersionMismatch`] saying which side must change.
    VersionMismatch,
    /// -32002: no thread has the id the request named.
    ThreadNotFound,
    /// -32003: the thread is known only from the record an earlier daemon
    /// wrote; this daemon has no session open for it, so it cannot take a
    /// prompt, a cancel, or steering.
    ThreadNotLoaded,
    /// -32004: the thread already has a turn running or waiting for input.
    /// Prompt again once it ends.
    ThreadBusy,
    /// -32005: the thread has no running model turn to steer.
    ThreadNotRunning,
    /// -32006: the running turn refused the steering input, as when its
    /// command queue is full.
    SteerRefused,
    /// -32007: the daemon is stopping and starts no new threads or turns.
    DaemonStopping,
    /// -32008: the daemon could not open the thread's session, for example
    /// because the workspace, the config, or the default agent is invalid.
    ThreadOpenFailed,
    /// -32009: the turn failed before its prompt was saved, for example
    /// because the provider's credentials could not be read.
    TurnFailed,
}

impl ErrorCode {
    /// The integer sent on the wire.
    pub const fn code(self) -> i64 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotFound => -32601,
            Self::InvalidParams => -32602,
            Self::InternalError => -32603,
            Self::HandshakeRequired => -32000,
            Self::VersionMismatch => -32001,
            Self::ThreadNotFound => -32002,
            Self::ThreadNotLoaded => -32003,
            Self::ThreadBusy => -32004,
            Self::ThreadNotRunning => -32005,
            Self::SteerRefused => -32006,
            Self::DaemonStopping => -32007,
            Self::ThreadOpenFailed => -32008,
            Self::TurnFailed => -32009,
        }
    }

    /// The known code for `code`, or `None` for a code this crate does not
    /// define (for example one added by a newer protocol minor).
    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            -32700 => Some(Self::ParseError),
            -32600 => Some(Self::InvalidRequest),
            -32601 => Some(Self::MethodNotFound),
            -32602 => Some(Self::InvalidParams),
            -32603 => Some(Self::InternalError),
            -32000 => Some(Self::HandshakeRequired),
            -32001 => Some(Self::VersionMismatch),
            -32002 => Some(Self::ThreadNotFound),
            -32003 => Some(Self::ThreadNotLoaded),
            -32004 => Some(Self::ThreadBusy),
            -32005 => Some(Self::ThreadNotRunning),
            -32006 => Some(Self::SteerRefused),
            -32007 => Some(Self::DaemonStopping),
            -32008 => Some(Self::ThreadOpenFailed),
            -32009 => Some(Self::TurnFailed),
            _ => None,
        }
    }
}
