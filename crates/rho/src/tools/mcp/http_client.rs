//! The one concrete Streamable HTTP client every MCP HTTP session runs on.
//!
//! rmcp's transport worker and client session loop are generic over the HTTP
//! client. Handing rmcp `reqwest::Client` for plain servers and
//! `AuthClient<reqwest::Client>` for OAuth servers compiled both twice.
//! Dispatching through this enum compiles them once, which cut rmcp's share
//! of the release binary from 1643 KB to 1508 KB (measured 2026-10-08).

use std::{collections::HashMap, sync::Arc};

use futures_util::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use rmcp::{
    model::ClientJsonRpcMessage,
    transport::{
        auth::AuthClient,
        streamable_http_client::{
            SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use sse_stream::Sse;

use super::oauth::{self, McpHttpClient};

type TransportError = StreamableHttpError<reqwest::Error>;

/// Not `Debug`: the authorized client holds live credentials.
#[derive(Clone)]
pub(super) enum McpTransportClient {
    Plain(reqwest::Client),
    Authorized(Box<AuthClient<reqwest::Client>>),
}

impl McpTransportClient {
    /// Build the client a session sends requests with from its authorization
    /// outcome.
    pub(super) fn new(client: McpHttpClient) -> anyhow::Result<Self> {
        Ok(match client {
            McpHttpClient::Default => Self::Plain(oauth::transport_http_client()?),
            McpHttpClient::Authorized(client) => Self::Authorized(client),
        })
    }
}

// Every method, including the `_with_max_sse_event_size` variants, delegates
// to the same method on the inner client. Falling back to the trait defaults
// would silently drop the transport's SSE event-size limit.
impl StreamableHttpClient for McpTransportClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, TransportError> {
        match self {
            Self::Plain(client) => {
                client
                    .post_message(uri, message, session_id, auth_header, custom_headers)
                    .await
            }
            Self::Authorized(client) => {
                client
                    .post_message(uri, message, session_id, auth_header, custom_headers)
                    .await
            }
        }
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, TransportError> {
        match self {
            Self::Plain(client) => {
                client
                    .post_message_with_max_sse_event_size(
                        uri,
                        message,
                        session_id,
                        auth_header,
                        custom_headers,
                        max_sse_event_size,
                    )
                    .await
            }
            Self::Authorized(client) => {
                client
                    .post_message_with_max_sse_event_size(
                        uri,
                        message,
                        session_id,
                        auth_header,
                        custom_headers,
                        max_sse_event_size,
                    )
                    .await
            }
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), TransportError> {
        match self {
            Self::Plain(client) => {
                client
                    .delete_session(uri, session_id, auth_header, custom_headers)
                    .await
            }
            Self::Authorized(client) => {
                client
                    .delete_session(uri, session_id, auth_header, custom_headers)
                    .await
            }
        }
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, TransportError> {
        match self {
            Self::Plain(client) => {
                client
                    .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
                    .await
            }
            Self::Authorized(client) => {
                client
                    .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
                    .await
            }
        }
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, TransportError> {
        match self {
            Self::Plain(client) => {
                client
                    .get_stream_with_max_sse_event_size(
                        uri,
                        session_id,
                        last_event_id,
                        auth_header,
                        custom_headers,
                        max_sse_event_size,
                    )
                    .await
            }
            Self::Authorized(client) => {
                client
                    .get_stream_with_max_sse_event_size(
                        uri,
                        session_id,
                        last_event_id,
                        auth_header,
                        custom_headers,
                        max_sse_event_size,
                    )
                    .await
            }
        }
    }
}
