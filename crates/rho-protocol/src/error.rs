//! Stable JSON-RPC error codes.

use schemars::{Schema, SchemaGenerator};

/// Declares [`ErrorCode`], [`ALL`], and each code's name and `data` type
/// from one list, so every variant is in the table [`ErrorCode::from_code`]
/// searches and [`crate::schema`] publishes. `=> Type` names the shape of a
/// code's `data`.
macro_rules! error_codes {
    (
        $(#[$enum_meta:meta])*
        pub enum ErrorCode {
            $($(#[$meta:meta])* $variant:ident = $code:literal $(=> $data:ty)?,)+
        }
    ) => {
        $(#[$enum_meta])*
        pub enum ErrorCode {
            $($(#[$meta])* $variant = $code,)+
        }

        /// Every [`ErrorCode`], which [`ErrorCode::from_code`] searches and
        /// [`crate::schema`] publishes.
        pub(crate) const ALL: &[ErrorCode] = &[$(ErrorCode::$variant,)+];

        impl ErrorCode {
            /// The variant's name, which the schema publishes with its code.
            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant),)+
                }
            }

            /// The schema of the error's `data`, for the codes whose `data`
            /// has a shape.
            pub(crate) fn data_schema(self, generator: &mut SchemaGenerator) -> Option<Schema> {
                match self {
                    $(Self::$variant => data_schema!(generator $($data)?),)+
                }
            }
        }
    };
}

macro_rules! data_schema {
    ($generator:ident) => {
        None
    };
    ($generator:ident $data:ty) => {
        Some($generator.subschema_for::<$data>())
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
        VersionMismatch = -32001 => crate::VersionMismatch,
        /// This daemon has not loaded that session, so it cannot take a
        /// prompt, a cancel, or steering. The session may not exist: telling
        /// the two apart comes with reopen (#1661).
        SessionNotLoaded = -32003,
        /// The session already has a turn running or waiting for input.
        /// Prompt again once it ends.
        SessionBusy = -32004,
        /// No model request is taking input.
        NotSteerable = -32005,
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
