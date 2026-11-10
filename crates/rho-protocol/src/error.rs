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
        /// `rho/hello` named an incompatible protocol version. `data` is a
        /// [`crate::VersionMismatch`] saying which side must change and, for
        /// a client newer than the daemon, what restarting the daemon would
        /// cost.
        VersionMismatch = -32001 => crate::VersionMismatch,
        /// No saved session has that id, in any workspace.
        SessionNotFound = -32003,
        /// The session already has a turn running or waiting for input, or
        /// a delete under way, or another job of the daemon's writes it, so
        /// it cannot be reopened (#1661); a delete also refuses while a
        /// settings change waits or applies, or background work or a run the
        /// session started is still going. Try again once that ends.
        SessionBusy = -32004,
        /// No model request is taking input.
        NotSteerable = -32005,
        /// The running turn refused the steering input, or a request to
        /// withdraw a steer, as when its command queue is full.
        SteerRefused = -32006,
        /// The daemon is stopping: it opens no new sessions and starts no
        /// new turns, and a prompt still waiting for its turn to start when
        /// the stop begins is answered with this.
        DaemonStopping = -32007,
        /// The daemon could not open the session, for example because the
        /// config is invalid, the agent a saved session was saved as changed
        /// or is gone, or the session is a subagent's or workflow node's,
        /// which only its parent continues (#1661).
        SessionOpenFailed = -32008,
        /// The turn ended before its prompt was saved, as when a cancel
        /// arrives before the turn begins, its time limit passes, or it
        /// fails first, for example because the provider's credentials
        /// could not be read. `data` is how it ended.
        TurnEnded = -32009 => crate::TurnOutcome,
        /// The session has no open input request with that id: another
        /// client answered it first, or the turn withdrew it, as when the
        /// turn was cancelled (#1660).
        InputNotOpen = -32011,
        /// Another rho process, such as the TUI, has the session open, or,
        /// for a delete, a session it started, so the daemon cannot reopen
        /// or delete it (#1661). Close it there first.
        SessionInUse = -32012,
        /// A variable in the daemon's environment holds the auth profile's
        /// credential, or `RHO_CREDENTIAL_STORE` chooses the credential
        /// store, and it wins over what the request would save, so saving
        /// or deleting it would change nothing (#1662). Unset it where the
        /// daemon starts and restart the daemon. `data` is a
        /// [`crate::EnvironmentOverride`] naming the variable.
        EnvironmentOverride = -32013 => crate::EnvironmentOverride,
        /// The provider refused the API key, so the daemon saved nothing
        /// (#1662).
        KeyRefused = -32014,
        /// The credential store could not be opened, read, or written, as
        /// when the OS keyring is locked or the daemon has no session D-Bus
        /// to reach it (#1662). `rho/daemon/status` says why, and
        /// `rho/auth/choose_store` chooses another store.
        CredentialStoreUnavailable = -32015,
        /// The daemon has not loaded the session, which a request that only
        /// changes a loaded session, such as a settings change, needs.
        /// Attach to the session first, which loads it.
        SessionNotLoaded = -32016,
        /// The daemon is already signing in to that auth profile, for this
        /// client or another (#1662). Wait for that login to finish, then
        /// start again.
        LoginBusy = -32017,
        /// No login under way has that id: it already ended, or never
        /// existed (#1662).
        LoginNotFound = -32018,
        /// The daemon could not open the session because its model's auth
        /// profile has no usable credentials: none are saved or set, the
        /// store could not read them, or the provider refused them. Sign in
        /// (`rho/auth/login` or `rho/auth/key`), then try again.
        SignInRequired = -32019,
        /// The prompting client's project trust (the `RHO_TRUST_PROJECT_*`
        /// variables in its [`crate::ClientEnvironment`]) is not the trust
        /// the session opened with, so the daemon started no turn (#1779):
        /// the session would run the prompt with trust this client did not
        /// grant, or without trust it did. The session, its turns, and its
        /// attached clients are left as they were. Open a session of the
        /// client's own, or prompt with the session's trust. `data` is a
        /// [`crate::ProjectTrustMismatch`] naming the variables that
        /// differ.
        ProjectTrustMismatch = -32020 => crate::ProjectTrustMismatch,
        /// The settings changed since the revision a `rho/config/set` was
        /// made against, or another writer holds the file, so the daemon
        /// wrote nothing. Read them again (`rho/config/get`), then send the
        /// change again.
        ConfigConflict = -32021,
        /// The settings file is not valid TOML, or does not load for keys
        /// the change leaves alone, so the daemon wrote nothing. Fix the
        /// file by hand, or change the keys it fails on; `rho/config/get`
        /// lists where it fails.
        ConfigInvalid = -32022,
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
