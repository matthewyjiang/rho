//! Token ownership follows rmcp's local session lifecycle, including internal closure.

use std::{collections::HashMap, sync::Mutex};

use futures_util::Stream;
use http::{header::HeaderValue, request::Parts};
use rmcp::{
    model::{ClientJsonRpcMessage, GetExtensions, ServerJsonRpcMessage},
    transport::streamable_http_server::session::{
        local::LocalSessionManager, ServerSseMessage, SessionId, SessionManager,
    },
};

/// A token name is a principal across secret rotation, not a cached grant or secret.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Owner(pub(super) String);

#[derive(Default)]
pub(super) struct Sessions {
    local: LocalSessionManager,
    owners: Mutex<HashMap<SessionId, Owner>>,
}

impl Sessions {
    /// Unknown IDs and IDs owned by another token make the same authorization decision.
    pub(super) fn owns(&self, id: &HeaderValue, owner: &Owner) -> bool {
        id.to_str().ok().is_some_and(|id| {
            self.owners
                .lock()
                .expect("MCP HTTP session owners lock poisoned")
                .get(id)
                == Some(owner)
        })
    }
}

impl SessionManager for Sessions {
    type Error = <LocalSessionManager as SessionManager>::Error;
    type Transport = <LocalSessionManager as SessionManager>::Transport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        self.local.create_session().await
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        let ClientJsonRpcMessage::Request(request) = &message else {
            unreachable!("rmcp only initializes sessions with an initialize request");
        };
        let owner = request
            .request
            .extensions()
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<Owner>())
            .expect("the HTTP boundary authenticates every initialize request")
            .clone();
        // Bind before initialization can return a session header or stream. Inserting
        // before awaiting also lets an internal close clean up an interrupted handshake.
        self.owners
            .lock()
            .expect("MCP HTTP session owners lock poisoned")
            .insert(id.clone(), owner);
        let result = self.local.initialize_session(id, message).await;
        if result.is_err() {
            self.owners
                .lock()
                .expect("MCP HTTP session owners lock poisoned")
                .remove(id);
        }
        result
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        self.local.has_session(id).await
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        // rmcp calls this for DELETE and when a session worker exits or expires.
        self.owners
            .lock()
            .expect("MCP HTTP session owners lock poisoned")
            .remove(id);
        self.local.close_session(id).await
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.local.create_stream(id, message).await
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        self.local.accept_message(id, message).await
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.local.create_standalone_stream(id).await
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.local.resume(id, last_event_id).await
    }
}
