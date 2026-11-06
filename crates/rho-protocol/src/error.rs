//! Stable JSON-RPC error codes.

/// Every error code the daemon sends. Codes are part of the wire contract and
/// never change meaning; protocol minors may add codes, so the enum is
/// non-exhaustive and [`crate::RpcError`] keeps the raw integer.
///
/// Each variant's discriminant is its wire integer, so the compiler rejects
/// two variants with one code. The first five are the JSON-RPC 2.0 reserved
/// codes. Rho-specific codes use the implementation-defined server range from
/// -32000 down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(i64)]
pub enum ErrorCode {
    /// The line was not valid JSON. Sent with a `null` id.
    ParseError = -32700,
    /// The line was JSON but not a valid JSON-RPC 2.0 request, or the
    /// request is not allowed in the connection's current state (a second
    /// `rho/hello`, or a line over [`crate::MAX_LINE_BYTES`]).
    InvalidRequest = -32600,
    /// The daemon does not implement the method.
    MethodNotFound = -32601,
    /// The method exists but its params do not match its schema.
    InvalidParams = -32602,
    /// The daemon failed while handling a valid request.
    InternalError = -32603,
    /// A request other than `rho/hello` arrived before the handshake
    /// completed. Send `rho/hello` first.
    HandshakeRequired = -32000,
    /// `rho/hello` named an incompatible protocol major. `data` is a
    /// [`crate::VersionMismatch`] saying which side must change.
    VersionMismatch = -32001,
    /// No thread has the id the request named.
    ThreadNotFound = -32002,
    /// The thread is known only from the record an earlier daemon
    /// wrote; this daemon has no session open for it, so it cannot take a
    /// prompt, a cancel, or steering.
    ThreadNotLoaded = -32003,
    /// The thread already has a turn running or waiting for input.
    /// Prompt again once it ends.
    ThreadBusy = -32004,
    /// The thread has no running model turn to steer.
    ThreadNotRunning = -32005,
    /// The running turn refused the steering input, as when its
    /// command queue is full.
    SteerRefused = -32006,
    /// The daemon is stopping: it starts no new threads or turns, and a turn
    /// it stopped before the turn's prompt was saved answers its prompt with
    /// this.
    DaemonStopping = -32007,
    /// The daemon could not open the thread's session, for example
    /// because the workspace, the config, or the default agent is invalid.
    ThreadOpenFailed = -32008,
    /// The turn failed before its prompt was saved, for example
    /// because the provider's credentials could not be read. The message
    /// says why.
    TurnFailed = -32009,
    /// The turn was cancelled before its prompt was saved, as when
    /// `rho/thread/cancel` arrives before the turn begins.
    TurnCancelled = -32010,
}

/// Every [`ErrorCode`], which [`ErrorCode::from_code`] searches.
const ALL: [ErrorCode; 16] = [
    ErrorCode::ParseError,
    ErrorCode::InvalidRequest,
    ErrorCode::MethodNotFound,
    ErrorCode::InvalidParams,
    ErrorCode::InternalError,
    ErrorCode::HandshakeRequired,
    ErrorCode::VersionMismatch,
    ErrorCode::ThreadNotFound,
    ErrorCode::ThreadNotLoaded,
    ErrorCode::ThreadBusy,
    ErrorCode::ThreadNotRunning,
    ErrorCode::SteerRefused,
    ErrorCode::DaemonStopping,
    ErrorCode::ThreadOpenFailed,
    ErrorCode::TurnFailed,
    ErrorCode::TurnCancelled,
];

impl ErrorCode {
    /// The integer sent on the wire.
    pub const fn code(self) -> i64 {
        self as i64
    }

    /// The known code for `code`, or `None` for a code this crate does not
    /// define (for example one added by a newer protocol minor).
    pub const fn from_code(code: i64) -> Option<Self> {
        let mut index = 0;
        while index < ALL.len() {
            if ALL[index].code() == code {
                return Some(ALL[index]);
            }
            index += 1;
        }
        None
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
