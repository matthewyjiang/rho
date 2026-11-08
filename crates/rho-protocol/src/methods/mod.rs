//! Method and notification names and their params and result types.
//!
//! Names are `rho/<area>/<verb>`, except the connection-level `rho/hello`.
//! Params and results ignore unknown fields so a protocol minor can add
//! optional fields without breaking older peers. Ids are strings.
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | [`HELLO`] | [`HelloParams`] | [`HelloResponse`] |
//! | [`DAEMON_STATUS`] | [`StatusParams`] | [`DaemonStatus`] |
//! | [`DAEMON_STOP`] | [`StopParams`] | [`StopResponse`] |
//! | [`SESSION_NEW`] | [`SessionNewParams`] | [`SessionNewResponse`] |
//! | [`SESSION_PROMPT`] | [`SessionPromptParams`] | [`SessionPromptResponse`] |
//! | [`SESSION_CANCEL`] | [`SessionCancelParams`] | [`SessionCancelResponse`] |
//! | [`SESSION_STEER`] | [`SessionSteerParams`] | [`SessionSteerResponse`] |
//! | [`SESSION_ANSWER`] | [`SessionAnswerParams`] | [`SessionAnswerResponse`] |
//! | [`SESSION_HOLD`] | [`SessionHoldParams`] | [`SessionHoldResponse`] |
//! | [`SESSION_ATTACH`] | [`SessionAttachParams`] | [`SessionAttachResponse`] |
//! | [`SESSION_DETACH`] | [`SessionDetachParams`] | [`SessionDetachResponse`] |
//! | [`JOB_LIST`] | [`JobListParams`] | [`JobListResponse`] |
//! | [`AUTH_KEY`] | [`AuthKeyParams`] | [`AuthKeyResponse`] |
//! | [`AUTH_LOGOUT`] | [`AuthLogoutParams`] | [`AuthLogoutResponse`] |
//!
//! The daemon sends these notifications; clients send none.
//!
//! | Notification | Params |
//! |---|---|
//! | [`SESSION_EVENT`] | [`SessionEventParams`] |
//! | [`SESSION_DETACHED`] | [`SessionDetachedParams`] |
//!
//! [`crate::schema`] publishes every method and notification as JSON Schema.
//! Enums that read values they do not know as `Unknown`, so a protocol minor
//! may add values, have their `Unknown` variant marked `x-rho-other` there,
//! where it matches every value the enum does not list. Absent optional
//! fields are omitted, never sent as `null`, so one schema describes both
//! directions.

use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

mod auth;
mod daemon;
mod event;
mod input;
mod job;
mod outcome;
mod session;

pub use auth::{
    AuthKeyParams, AuthKeyResponse, AuthLogoutParams, AuthLogoutResponse, EnvironmentOverride,
    UncheckedKey, UncheckedReason, AUTH_KEY, AUTH_LOGOUT,
};
pub use daemon::{
    ClientInfo, DaemonInfo, DaemonStatus, HelloParams, HelloResponse, StatusParams, StopParams,
    StopResponse, DAEMON_STATUS, DAEMON_STOP, HELLO,
};
pub use event::{
    CardTone, CompactionId, CompactionStatus, ContentBlock, Cost, Cursor, Location, MessageId,
    PlanEntry, PlanStatus, Seq, SessionEvent, StreamId, ToolCallId, ToolCallUpdate, ToolContent,
    ToolKind, ToolStatus,
};
pub use input::{
    ApprovalAsk, ApprovalSource, Ask, Choice, DefaultAnswer, DefaultStyle, InputAnswer,
    InputRequest, InputRequestId, InputTimeout, Operation, PathScope, PermissionMode,
    PlanApprovalAsk, Question, QuestionAsk, Resolution, Selection, SessionAnswerParams,
    SessionAnswerResponse, SessionHoldParams, SessionHoldResponse, SESSION_ANSWER, SESSION_HOLD,
};
pub use job::{
    Budget, JobId, JobKind, JobListParams, JobListResponse, JobState, JobSummary, QueuePlace,
    JOB_LIST,
};
pub use outcome::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome};
pub use session::{
    ClientEnvironment, DetachReason, Replay, SessionAttachParams, SessionAttachResponse,
    SessionCancelParams, SessionCancelResponse, SessionDetachParams, SessionDetachResponse,
    SessionDetachedParams, SessionEventParams, SessionId, SessionNewParams, SessionNewResponse,
    SessionPromptParams, SessionPromptResponse, SessionSteerParams, SessionSteerResponse,
    SESSION_ATTACH, SESSION_CANCEL, SESSION_DETACH, SESSION_DETACHED, SESSION_EVENT, SESSION_NEW,
    SESSION_PROMPT, SESSION_STEER,
};

/// A request method with typed params and result.
///
/// Implemented by the zero-sized markers this module declares, so
/// [`crate::Client::request`] can check a call's types at compile time. Each
/// pairs `NAME` with the params and result types the daemon uses for that
/// method.
pub trait Method {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned + JsonSchema;
    type Result: Serialize + DeserializeOwned + JsonSchema;
}

/// A notification the daemon sends, typed like [`Method`].
///
/// Implemented by the zero-sized markers this module declares, so the
/// daemon encodes, and [`crate::Client`] decodes, each notification with one
/// params type.
pub trait Notification {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned + JsonSchema;
}

/// What [`visit`] calls for every method and notification; implemented by
/// [`crate::schema`] to publish them.
pub(crate) trait Visitor {
    fn method<M: Method>(&mut self);
    fn notification<N: Notification>(&mut self);
}

/// Declares the method and notification markers and [`visit`] from one list,
/// so a method cannot exist without being published (#1657).
macro_rules! protocol {
    (
        methods {
            $($method:ident = $method_name:ident: $params:ty => $result:ty,)+
        }
        notifications {
            $($notification:ident = $notification_name:ident: $notification_params:ty,)+
        }
    ) => {
        $(
            #[doc = concat!("Marker for [`", stringify!($method_name), "`].")]
            pub struct $method;

            impl Method for $method {
                const NAME: &'static str = $method_name;
                type Params = $params;
                type Result = $result;
            }
        )+
        $(
            #[doc = concat!("Marker for the [`", stringify!($notification_name), "`] notification.")]
            pub struct $notification;

            impl Notification for $notification {
                const NAME: &'static str = $notification_name;
                type Params = $notification_params;
            }
        )+

        /// Calls `visitor` for every method, then every notification.
        pub(crate) fn visit(visitor: &mut impl Visitor) {
            $(visitor.method::<$method>();)+
            $(visitor.notification::<$notification>();)+
        }
    };
}

protocol! {
    methods {
        Hello = HELLO: HelloParams => HelloResponse,
        Status = DAEMON_STATUS: StatusParams => DaemonStatus,
        Stop = DAEMON_STOP: StopParams => StopResponse,
        SessionNew = SESSION_NEW: SessionNewParams => SessionNewResponse,
        SessionPrompt = SESSION_PROMPT: SessionPromptParams => SessionPromptResponse,
        SessionCancel = SESSION_CANCEL: SessionCancelParams => SessionCancelResponse,
        SessionSteer = SESSION_STEER: SessionSteerParams => SessionSteerResponse,
        SessionAnswer = SESSION_ANSWER: SessionAnswerParams => SessionAnswerResponse,
        SessionHold = SESSION_HOLD: SessionHoldParams => SessionHoldResponse,
        SessionAttach = SESSION_ATTACH: SessionAttachParams => SessionAttachResponse,
        SessionDetach = SESSION_DETACH: SessionDetachParams => SessionDetachResponse,
        JobList = JOB_LIST: JobListParams => JobListResponse,
        AuthKey = AUTH_KEY: AuthKeyParams => AuthKeyResponse,
        AuthLogout = AUTH_LOGOUT: AuthLogoutParams => AuthLogoutResponse,
    }
    notifications {
        SessionEventNotification = SESSION_EVENT: SessionEventParams,
        SessionDetachedNotification = SESSION_DETACHED: SessionDetachedParams,
    }
}
