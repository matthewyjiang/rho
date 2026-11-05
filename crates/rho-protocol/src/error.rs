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
            _ => None,
        }
    }
}
