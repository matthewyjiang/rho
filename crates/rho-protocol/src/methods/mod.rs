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
//! | [`DAEMON_RESTART`] | [`RestartParams`] | [`RestartResponse`] |
//! | [`SESSION_NEW`] | [`SessionNewParams`] | [`SessionNewResponse`] |
//! | [`SESSION_PROMPT`] | [`SessionPromptParams`] | [`SessionPromptResponse`] |
//! | [`SESSION_CANCEL`] | [`SessionCancelParams`] | [`SessionCancelResponse`] |
//! | [`SESSION_STEER`] | [`SessionSteerParams`] | [`SessionSteerResponse`] |
//! | [`SESSION_UNSTEER`] | [`SessionUnsteerParams`] | [`SessionUnsteerResponse`] |
//! | [`SESSION_ANSWER`] | [`SessionAnswerParams`] | [`SessionAnswerResponse`] |
//! | [`SESSION_HOLD`] | [`SessionHoldParams`] | [`SessionHoldResponse`] |
//! | [`SESSION_ATTACH`] | [`SessionAttachParams`] | [`SessionAttachResponse`] |
//! | [`SESSION_OBSERVE`] | [`SessionObserveParams`] | [`SessionAttachResponse`] |
//! | [`SESSION_DETACH`] | [`SessionDetachParams`] | [`SessionDetachResponse`] |
//! | [`SESSION_MCP`] | [`SessionMcpParams`] | [`SessionMcpInventory`] |
//! | [`SESSION_MCP_CATALOG`] | [`SessionMcpCatalogParams`] | [`SessionMcpCatalog`] |
//! | [`SESSION_MCP_RESOURCE`] | [`SessionMcpResourceParams`] | [`SessionMcpResourceResponse`] |
//! | [`SESSION_MCP_COMPLETE`] | [`SessionMcpCompleteParams`] | [`SessionMcpCompleteResponse`] |
//! | [`SESSION_PROCESSES`] | [`SessionProcessesParams`] | [`SessionProcessesResponse`] |
//! | [`SESSION_INSPECT`] | [`SessionInspectParams`] | [`SessionInspectResponse`] |
//! | [`SESSION_CONFIGURE`] | [`SessionConfigureParams`] | [`SessionConfigureResponse`] |
//! | [`SESSION_LIST`] | [`SessionListParams`] | [`SessionListResponse`] |
//! | [`SESSION_RENAME`] | [`SessionRenameParams`] | [`SessionRenameResponse`] |
//! | [`SESSION_DELETE`] | [`SessionDeleteParams`] | [`SessionDeleteResponse`] |
//! | [`JOB_LIST`] | [`JobListParams`] | [`JobListResponse`] |
//! | [`AUTH_LIST`] | [`AuthListParams`] | [`AuthListResponse`] |
//! | [`AUTH_KEY`] | [`AuthKeyParams`] | [`AuthKeyResponse`] |
//! | [`AUTH_LOGOUT`] | [`AuthLogoutParams`] | [`AuthLogoutResponse`] |
//! | [`AUTH_LOGIN`] | [`AuthLoginParams`] | [`AuthLoginResponse`] |
//! | [`AUTH_LOGIN_CANCEL`] | [`AuthLoginCancelParams`] | [`AuthLoginCancelResponse`] |
//! | [`AUTH_CHOOSE_STORE`] | [`AuthChooseStoreParams`] | [`AuthChooseStoreResponse`] |
//! | [`CONFIG_SCHEMA`] | [`ConfigSchemaParams`] | [`ConfigSchema`] |
//! | [`CONFIG_REGISTER`] | [`ConfigRegisterParams`] | [`ConfigRegisterResponse`] |
//! | [`CONFIG_GET`] | [`ConfigGetParams`] | [`ConfigGetResponse`] |
//! | [`CONFIG_SET`] | [`ConfigSetParams`] | [`ConfigSetResponse`] |
//!
//! The daemon sends these notifications; clients send none.
//!
//! | Notification | Params |
//! |---|---|
//! | [`SESSION_EVENT`] | [`SessionEventParams`] |
//! | [`SESSION_DETACHED`] | [`SessionDetachedParams`] |
//! | [`AUTH_LOGIN_PROMPT`] | [`AuthLoginPromptParams`] |
//! | [`AUTH_LOGIN_FINISHED`] | [`AuthLoginFinishedParams`] |
//! | [`CONFIG_CHANGED`] | [`ConfigChangedParams`] |
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
mod catalog;
mod config;
mod daemon;
mod event;
mod goal;
mod grant;
mod input;
mod inspect;
mod job;
mod login;
mod mcp;
mod options;
mod outcome;
mod process;
mod session;
mod setup;
mod store;

pub use auth::{
    AuthKeyParams, AuthKeyResponse, AuthKind, AuthListParams, AuthListResponse, AuthLogoutParams,
    AuthLogoutResponse, AuthStatus, AuthSummary, EnvironmentOverride, UncheckedKey,
    UncheckedReason, AUTH_KEY, AUTH_LIST, AUTH_LOGOUT,
};
pub use catalog::{
    SessionDeleteParams, SessionDeleteResponse, SessionListParams, SessionListResponse,
    SessionRenameParams, SessionRenameResponse, SessionSummary, SESSION_DELETE, SESSION_LIST,
    SESSION_RENAME,
};
pub use config::{
    Applies, ClientName, ConfigCategory, ConfigChangedParams, ConfigError, ConfigGetParams,
    ConfigGetResponse, ConfigLayer, ConfigNamespace, ConfigRegisterParams, ConfigRegisterResponse,
    ConfigRevision, ConfigSchema, ConfigSchemaParams, ConfigSetParams, ConfigSetResponse,
    ConfigWriter, FileOnlyTable, IntegerUnit, InvalidClientName, ModelRuntime, ModelSelection,
    SettingChange, SettingChoice, SettingEntry, SettingKey, SettingKind, SettingSpec, SettingValue,
    ValueSource, CONFIG_CHANGED, CONFIG_GET, CONFIG_REGISTER, CONFIG_SCHEMA, CONFIG_SET,
};
pub use daemon::{
    ClientCapability, ClientHello, ClientInfo, ClientRole, DaemonInfo, DaemonStatus, HelloParams,
    HelloResponse, PendingUpgrade, RestartParams, RestartResponse, StatusParams, StopParams,
    StopResponse, DAEMON_RESTART, DAEMON_STATUS, DAEMON_STOP, HELLO,
};
pub use event::{
    CardTone, CompactionCounts, CompactionId, CompactionStatus, CompactionTrigger, ContentBlock,
    Cursor, IdempotencyKey, Location, MessageId, PlanEntry, PlanStatus, RetryReason, Seq,
    SessionEvent, SteerId, StreamId, ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
    TurnId,
};
pub use goal::{SessionGoal, SessionGoalAction, SessionGoalPhase, SessionGoalStep};
pub use grant::{GrantAccess, GrantWorkspaces, SessionGrant};
pub use input::{
    ApprovalAsk, ApprovalSource, Ask, Choice, DefaultAnswer, DefaultStyle, InputAnswer, InputAsker,
    InputRequest, InputRequestId, InputTimeout, Operation, PathScope, PermissionMode,
    PlanApprovalAsk, Question, QuestionAsk, Resolution, Selection, SessionAnswerParams,
    SessionAnswerResponse, SessionControlAction, SessionHoldParams, SessionHoldResponse,
    SESSION_ANSWER, SESSION_HOLD,
};
pub use inspect::{SessionInspectParams, SessionInspectResponse, SessionReport, SESSION_INSPECT};
pub use job::{
    Budget, JobId, JobKind, JobListParams, JobListResponse, JobState, JobSummary, QueuePlace,
    JOB_LIST,
};
pub use login::{
    AuthLoginCancelParams, AuthLoginCancelResponse, AuthLoginFinishedParams, AuthLoginParams,
    AuthLoginPromptParams, AuthLoginResponse, LoginFlow, LoginId, LoginOutcome, LoginPrompt,
    AUTH_LOGIN, AUTH_LOGIN_CANCEL, AUTH_LOGIN_FINISHED, AUTH_LOGIN_PROMPT,
};
pub use mcp::{
    SessionMcpCatalog, SessionMcpCatalogParams, SessionMcpCompleteParams,
    SessionMcpCompleteResponse, SessionMcpInventory, SessionMcpMode, SessionMcpParams,
    SessionMcpPrompt, SessionMcpPromptArgument, SessionMcpResource, SessionMcpResourceContent,
    SessionMcpResourceParams, SessionMcpResourceResponse, SessionMcpServer, SessionMcpStatus,
    SESSION_MCP, SESSION_MCP_CATALOG, SESSION_MCP_COMPLETE, SESSION_MCP_RESOURCE,
};
pub use options::{
    CommandKind, FastModeOptions, ModelChoice, ModelOptions, ModelRef, PermissionModeOptions,
    ReasoningLevel, ReasoningOptions, SessionChange, SessionCommand, SessionConfigureParams,
    SessionConfigureResponse, SessionOptions, SESSION_CONFIGURE,
};
pub use outcome::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome};
pub use process::{
    ProcessChunk, ProcessOutput, ProcessOutputParams, ProcessState, ProcessStream, SessionProcess,
    SessionProcessesParams, SessionProcessesResponse, SESSION_PROCESSES,
};
pub use session::{
    ClientEnvironment, DetachReason, ProjectTrustMismatch, Replay, SessionAttachParams,
    SessionAttachResponse, SessionCancelParams, SessionCancelResponse, SessionDetachParams,
    SessionDetachResponse, SessionDetachedParams, SessionEventParams, SessionId, SessionNewParams,
    SessionNewResponse, SessionObserveParams, SessionPromptParams, SessionPromptResponse,
    SessionSteerParams, SessionSteerResponse, SessionUnsteerParams, SessionUnsteerResponse,
    UnsteerOutcome, SESSION_ATTACH, SESSION_CANCEL, SESSION_DETACH, SESSION_DETACHED,
    SESSION_EVENT, SESSION_NEW, SESSION_OBSERVE, SESSION_PROMPT, SESSION_STEER, SESSION_UNSTEER,
};
pub use setup::{ChildThreads, IdleWake, McpServer, SecretMap, SessionPart, SessionSetup, SignIn};
pub use store::{
    AuthChooseStoreParams, AuthChooseStoreResponse, CredentialStoreKind, CredentialStoreStatus,
    StoreSelectedBy, AUTH_CHOOSE_STORE,
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
        Restart = DAEMON_RESTART: RestartParams => RestartResponse,
        SessionNew = SESSION_NEW: SessionNewParams => SessionNewResponse,
        SessionPrompt = SESSION_PROMPT: SessionPromptParams => SessionPromptResponse,
        SessionCancel = SESSION_CANCEL: SessionCancelParams => SessionCancelResponse,
        SessionSteer = SESSION_STEER: SessionSteerParams => SessionSteerResponse,
        SessionUnsteer = SESSION_UNSTEER: SessionUnsteerParams => SessionUnsteerResponse,
        SessionAnswer = SESSION_ANSWER: SessionAnswerParams => SessionAnswerResponse,
        SessionHold = SESSION_HOLD: SessionHoldParams => SessionHoldResponse,
        SessionAttach = SESSION_ATTACH: SessionAttachParams => SessionAttachResponse,
        SessionObserve = SESSION_OBSERVE: SessionObserveParams => SessionAttachResponse,
        SessionDetach = SESSION_DETACH: SessionDetachParams => SessionDetachResponse,
        SessionConfigure = SESSION_CONFIGURE: SessionConfigureParams => SessionConfigureResponse,
        SessionMcp = SESSION_MCP: SessionMcpParams => SessionMcpInventory,
        SessionMcpCatalogRead = SESSION_MCP_CATALOG: SessionMcpCatalogParams => SessionMcpCatalog,
        SessionMcpResourceRead = SESSION_MCP_RESOURCE: SessionMcpResourceParams => SessionMcpResourceResponse,
        SessionMcpComplete = SESSION_MCP_COMPLETE: SessionMcpCompleteParams => SessionMcpCompleteResponse,
        SessionProcesses = SESSION_PROCESSES: SessionProcessesParams => SessionProcessesResponse,
        SessionInspect = SESSION_INSPECT: SessionInspectParams => SessionInspectResponse,
        SessionList = SESSION_LIST: SessionListParams => SessionListResponse,
        SessionRename = SESSION_RENAME: SessionRenameParams => SessionRenameResponse,
        SessionDelete = SESSION_DELETE: SessionDeleteParams => SessionDeleteResponse,
        JobList = JOB_LIST: JobListParams => JobListResponse,
        AuthList = AUTH_LIST: AuthListParams => AuthListResponse,
        AuthKey = AUTH_KEY: AuthKeyParams => AuthKeyResponse,
        AuthLogout = AUTH_LOGOUT: AuthLogoutParams => AuthLogoutResponse,
        AuthLogin = AUTH_LOGIN: AuthLoginParams => AuthLoginResponse,
        AuthLoginCancel = AUTH_LOGIN_CANCEL: AuthLoginCancelParams => AuthLoginCancelResponse,
        AuthChooseStore = AUTH_CHOOSE_STORE: AuthChooseStoreParams => AuthChooseStoreResponse,
        ConfigSchemaMethod = CONFIG_SCHEMA: ConfigSchemaParams => ConfigSchema,
        ConfigRegister = CONFIG_REGISTER: ConfigRegisterParams => ConfigRegisterResponse,
        ConfigGet = CONFIG_GET: ConfigGetParams => ConfigGetResponse,
        ConfigSet = CONFIG_SET: ConfigSetParams => ConfigSetResponse,
    }
    notifications {
        SessionEventNotification = SESSION_EVENT: SessionEventParams,
        SessionDetachedNotification = SESSION_DETACHED: SessionDetachedParams,
        AuthLoginPromptNotification = AUTH_LOGIN_PROMPT: AuthLoginPromptParams,
        AuthLoginFinishedNotification = AUTH_LOGIN_FINISHED: AuthLoginFinishedParams,
        ConfigChangedNotification = CONFIG_CHANGED: ConfigChangedParams,
    }
}
