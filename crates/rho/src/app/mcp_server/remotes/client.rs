//! One tool call on a remote `rho mcp serve`, on a connection of its own:
//! connect with the hub's secret, initialize, call, close. A connection per
//! call keeps no state to go stale, applies a `rho mcp remote add` from the
//! next call, and never queues one call's long `wait` behind another's.
//!
//! Every call is marked forwarded ([`FORWARDED`]): the remote serves it on
//! its own sessions and routes it nowhere, so hubs that list each other as
//! remotes never pass a call around. A hub's watch marks its calls as such
//! ([`Asked::ByWatch`]), and the remote answers them from a daemon already
//! running, as the hub's own watches do. A remote marks an error that
//! asking again will not change ([`LASTING`]).

use std::time::Duration;

use rho_protocol::SessionId;
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, ClientRequest, ContentBlock, RequestMetaObject,
        ServerResult,
    },
    service::PeerRequestOptions,
};
use serde_json::{Map, Value};

use super::{check_url, Remote};
use crate::{
    app::session_control::{Action, WaitResult},
    caller_context::CallerContext,
    tools::mcp::bearer_client::{connect_bearer, BearerError},
};

/// The `_meta` key that marks a call a hub forwarded: `"call"` for a
/// caller's, `"watch"` for one a watch makes on nobody's call.
pub(crate) const FORWARDED: &str = "rho/forwarded";

/// The `_meta` key of an error result that marks it lasting: the remote
/// refused the call, or knows no such session.
pub(crate) const LASTING: &str = "rho/lasting";

/// Who asked for a call a hub forwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    /// A caller, whose call may start the remote's daemon, as one on the
    /// remote itself may.
    ByCall,
    /// A watch, on nobody's call: it never starts a daemon a person
    /// stopped.
    ByWatch,
}

impl Asked {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ByCall => "call",
            Self::ByWatch => "watch",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [Self::ByCall, Self::ByWatch]
            .into_iter()
            .find(|asked| asked.name() == name)
    }
}

/// Whether asking a remote again could change an error it answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lasts {
    /// It refused the call, or knows no such session.
    ForGood,
    /// Anything else, as its daemon down.
    ForNow,
}

/// The longest one call may take, connecting included.
//
// Tripwire. The longest call a remote answers on purpose is a `wait` of up
// to 50 s (`MAX_WAIT_SECS`); the rest is connecting and initializing, a
// few round trips. Twenty seconds over that covers a slow link, while a
// hung remote still frees its caller.
const CALL_TIMEOUT: Duration = Duration::from_secs(70);

/// Why a call on a remote failed. Its `Display` is what the caller sees.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemoteError {
    /// The remote did not answer the call: down, unreachable, or too slow.
    /// Trying again later may work.
    #[error("could not reach server {server:?}: {detail}")]
    Unreachable { server: String, detail: String },
    /// The remote refused the hub's secret: its token for the hub was
    /// revoked or rotated.
    #[error(
        "server {server:?} refused this server's secret (401 Unauthorized); add it again with a new token from it"
    )]
    Refused { server: String },
    /// The remote answered, refusing or failing the call as `message` says.
    #[error("server {server:?}: {message}")]
    Failed {
        server: String,
        message: String,
        lasts: Lasts,
    },
}

/// Calls `action` with `arguments` on `remote`, as `asked`, for a caller in
/// `context`, which the remote reads to hold its sessions to the caller's
/// depth and access. Returns the tool's JSON result.
pub(crate) async fn call(
    remote: &Remote,
    action: Action,
    arguments: Map<String, Value>,
    context: &CallerContext,
    asked: Asked,
) -> Result<Value, RemoteError> {
    let unreachable = |detail: String| RemoteError::Unreachable {
        server: remote.name.clone(),
        detail,
    };
    let outcome = tokio::time::timeout(CALL_TIMEOUT, async {
        let endpoint = check_url(&remote.url)
            .await
            .map_err(|error| unreachable(format!("{error:#}")))?;
        let connection = connect_bearer(
            &remote.name,
            &endpoint.url,
            &endpoint.pinned,
            &remote.secret,
        )
        .await
        .map_err(|error| match error {
            BearerError::Unauthorized => RemoteError::Refused {
                server: remote.name.clone(),
            },
            BearerError::Failed(detail) => unreachable(detail),
        })?;
        let mut meta = Map::from_iter([(FORWARDED.to_owned(), Value::from(asked.name()))]);
        if connection
            .peer_info()
            .is_some_and(|info| CallerContext::accepted_by(&info.capabilities))
        {
            meta.extend(context.to_meta());
        }
        let options = PeerRequestOptions::no_options().with_meta(RequestMetaObject(meta.into()));
        let params = CallToolRequestParams::new(action.name()).with_arguments(arguments);
        let response = match connection
            .send_request_with_option(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                options,
            )
            .await
        {
            Ok(handle) => handle.await_response().await,
            Err(error) => Err(error),
        };
        // Closed off the call's path: the answer is in, and closing can wait
        // seconds on the remote, which must not cost the caller the answer.
        let server = remote.name.clone();
        tokio::spawn(async move {
            if let Err(error) = connection.cancel().await {
                tracing::debug!(%error, server, "could not close a remote connection");
            }
        });
        match response {
            Ok(ServerResult::CallToolResult(result)) => {
                let text = text_of(&result.content);
                if result.is_error == Some(true) {
                    let lasting = result.meta.as_ref().and_then(|meta| meta.0.get(LASTING))
                        == Some(&Value::Bool(true));
                    return Err(RemoteError::Failed {
                        server: remote.name.clone(),
                        message: text,
                        lasts: if lasting {
                            Lasts::ForGood
                        } else {
                            Lasts::ForNow
                        },
                    });
                }
                Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
            }
            Ok(_) => Err(unreachable(
                "it answered tools/call with something other than a tool result".to_owned(),
            )),
            Err(error) => Err(unreachable(error.to_string())),
        }
    })
    .await;
    outcome.unwrap_or_else(|_| {
        Err(unreachable(format!(
            "no answer within CALL_TIMEOUT ({} s)",
            CALL_TIMEOUT.as_secs()
        )))
    })
}

/// `wait` on `remote`, as `asked`: where `session` is once it is done, or
/// after `timeout_secs`, from `cursor`.
pub(crate) async fn wait(
    remote: &Remote,
    session: &SessionId,
    timeout_secs: u64,
    cursor: Option<&str>,
    context: &CallerContext,
    asked: Asked,
) -> Result<WaitResult, RemoteError> {
    let mut arguments = Map::new();
    arguments.insert("session".to_owned(), Value::from(session.as_str()));
    arguments.insert("timeout_secs".to_owned(), Value::from(timeout_secs));
    if let Some(cursor) = cursor {
        arguments.insert("cursor".to_owned(), Value::from(cursor));
    }
    let answer = call(remote, Action::Wait, arguments, context, asked).await?;
    // Another version's answer, perhaps: the next one may parse.
    serde_json::from_value(answer).map_err(|error| RemoteError::Failed {
        server: remote.name.clone(),
        message: format!("its `wait` answered something else: {error}"),
        lasts: Lasts::ForNow,
    })
}

/// The text blocks of a result, as one string: a rho server answers with
/// one, its JSON or its error.
fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}
