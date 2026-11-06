//! Stable JSON-RPC error codes.

/// Declares [`ErrorCode`] and [`ALL`] from one list, so every variant is
/// in the table [`ErrorCode::from_code`] searches.
macro_rules! error_codes {
    (
        $(#[$enum_meta:meta])*
        pub enum ErrorCode {
            $($(#[$meta:meta])* $variant:ident = $code:literal,)+
        }
    ) => {
        $(#[$enum_meta])*
        pub enum ErrorCode {
            $($(#[$meta])* $variant = $code,)+
        }

        /// Every [`ErrorCode`], which [`ErrorCode::from_code`] searches.
        const ALL: &[ErrorCode] = &[$(ErrorCode::$variant,)+];
    };
}

error_codes! {
    /// Every error code the daemon sends. Codes are part of the wire contract
    /// and never change meaning; protocol minors may add codes, so the enum
    /// is non-exhaustive and [`crate::RpcError`] keeps the raw integer.
    ///
    /// Each variant's discriminant is its wire integer, so the compiler
    /// rejects two variants with one code. The first five are the JSON-RPC
    /// 2.0 reserved codes. Rho-specific codes use the implementation-defined
    /// server range from -32000 down.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    #[repr(i64)]
    pub enum ErrorCode {
        /// The line was not valid JSON. Sent with a `null` id.
        ParseError = -32700,
        /// The line was JSON but not a valid JSON-RPC 2.0 request, or the
        /// request is not allowed in the connection's current state (a
        /// second `rho/hello`, or a line over [`crate::MAX_LINE_BYTES`]).
        InvalidRequest = -32600,
        /// The daemon does not implement the method.
        MethodNotFound = -32601,
        /// The method exists but its params do not match its schema, or a
        /// param is invalid, such as a session's `cwd` that is not an
        /// existing directory.
        InvalidParams = -32602,
        /// The daemon failed while handling a valid request.
        InternalError = -32603,
        /// A request other than `rho/hello` arrived before the handshake
        /// completed. Send `rho/hello` first.
        HandshakeRequired = -32000,
        /// `rho/hello` named an incompatible protocol major. `data` is a
        /// [`crate::VersionMismatch`] saying which side must change.
        VersionMismatch = -32001,
        /// No session has the id the request named: the daemon has none
        /// loaded and the session store has none saved.
        SessionNotFound = -32002,
        /// The session exists but this daemon has not loaded it, so it
        /// cannot take a prompt, a cancel, or steering.
        SessionNotLoaded = -32003,
        /// The session already has a turn running or waiting for input.
        /// Prompt again once it ends.
        SessionBusy = -32004,
        /// The session has no running model turn to steer.
        TurnNotRunning = -32005,
        /// The running turn refused the steering input, as when its
        /// command queue is full.
        SteerRefused = -32006,
        /// The daemon is stopping: it opens no new sessions and starts no
        /// new turns, and a prompt still waiting for its turn to start when
        /// the stop begins is answered with this.
        DaemonStopping = -32007,
        /// The daemon could not open the session, for example because the
        /// config or the default agent is invalid.
        SessionOpenFailed = -32008,
        /// The turn failed before its prompt was saved, for example
        /// because the provider's credentials could not be read. The
        /// message says why.
        TurnFailed = -32009,
        /// The turn was cancelled before its prompt was saved, as when
        /// `rho/session/cancel` arrives before the turn begins.
        TurnCancelled = -32010,
    }
}

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
