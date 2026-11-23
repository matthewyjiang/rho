//! A bare MCP client connection to a Streamable HTTP server with a static
//! bearer secret: no roots, no elicitation, no progress, no OAuth. A hub
//! `rho mcp serve` reaches its remotes with it (`app::mcp_server::remotes`).
//!
//! It runs on the handler and HTTP client types every configured server
//! runs on, so rmcp's client loop and transport compile once
//! (`http_client.rs`).

use rmcp::{
    service::{ClientInitializeError, RunningService},
    transport::{
        streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
        StreamableHttpClientTransport,
    },
    RoleClient, ServiceExt,
};

use super::{
    client::{McpClientHandler, McpClientServices},
    elicitation::{McpElicitationService, McpElicitationSupport},
    http_client::McpTransportClient,
    inflight::McpInFlightCalls,
    oauth,
    progress::McpProgressRouter,
    roots::McpRoots,
};

/// An initialized connection; `cancel` it to close.
pub(crate) type BearerConnection = RunningService<RoleClient, McpClientHandler>;

/// Why a connection could not be made.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BearerError {
    /// The server refused the secret.
    #[error("it refused the secret (401 Unauthorized)")]
    Unauthorized,
    /// Anything else: the network, the server, or its answer.
    #[error("{0}")]
    Failed(String),
}

/// Connects to `url`, sending `secret` as `Authorization: Bearer <secret>`,
/// and initializes. `identity` names the server in logs. Redirects are
/// never followed, so the secret never reaches another origin, and plain
/// `http`, which a caller allows only on a private network, bypasses any
/// proxy, which would carry the secret off it. When `pinned` names
/// addresses, the URL's host name connects only to those, the ones the
/// caller checked, rather than to whatever it resolves to now.
pub(crate) async fn connect_bearer(
    identity: &str,
    url: &url::Url,
    pinned: &[std::net::SocketAddr],
    secret: &str,
) -> Result<BearerConnection, BearerError> {
    // Server-initiated list changes have no session to act on here.
    let (events, _) = tokio::sync::mpsc::unbounded_channel();
    let handler = McpClientHandler::new(
        identity,
        McpRoots::default(),
        McpProgressRouter::new(),
        events,
        McpClientServices {
            elicit: McpElicitationService::new(
                identity,
                McpInFlightCalls::new(),
                McpElicitationSupport::Unavailable,
            ),
        },
    );
    let client = if url.scheme() == "http" {
        let mut builder = crate::reqwest_client_builder()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if let (Some(domain), false) = (url.domain(), pinned.is_empty()) {
            builder = builder.resolve_to_addrs(domain, pinned);
        }
        builder
            .build()
            .map_err(|error| BearerError::Failed(error.to_string()))?
    } else {
        oauth::transport_http_client().map_err(|error| BearerError::Failed(format!("{error:#}")))?
    };
    let client = McpTransportClient::Plain(client);
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .auth_header(secret.to_owned());
    let transport = StreamableHttpClientTransport::with_client(client, config);
    handler.serve(transport).await.map_err(describe)
}

/// What went wrong connecting, without rmcp's transport type names: a
/// refused secret as such, any other HTTP failure as itself.
fn describe(error: ClientInitializeError) -> BearerError {
    let ClientInitializeError::TransportError { error, .. } = &error else {
        return BearerError::Failed(error.to_string());
    };
    match error
        .error
        .downcast_ref::<StreamableHttpError<reqwest::Error>>()
    {
        Some(StreamableHttpError::AuthRequired(_)) => BearerError::Unauthorized,
        // Not a `source` of its wrapper, so its causes are reached from it.
        Some(StreamableHttpError::Client(client)) => BearerError::Failed(with_causes(client)),
        Some(other) => BearerError::Failed(with_causes(other)),
        None => BearerError::Failed(with_causes(error.error.as_ref())),
    }
}

/// `error` and the causes under it, as `reqwest`'s bare "error sending
/// request" says nothing of why: "connection refused" or a timeout is
/// further down. A cause the text already shows is not repeated.
fn with_causes(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        let next_text = next.to_string();
        if !text.contains(&next_text) {
            text.push_str(": ");
            text.push_str(&next_text);
        }
        cause = next.source();
    }
    text
}
