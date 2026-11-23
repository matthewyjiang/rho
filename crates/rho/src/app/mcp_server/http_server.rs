//! `rho mcp serve --http`: the tools over rmcp's streamable HTTP service,
//! for hosts that reach MCP servers by URL through a tunnel.
//!
//! What this layer guards, in order:
//! - it binds a loopback address, which the network reaches only through a
//!   tunnel the user set up, or an address on a private network
//!   (`app::http_guard`), such as an overlay network's, where the owner
//!   decided the network keeps plain HTTP private;
//! - every request carries a bearer token from `rho mcp token`, read from the
//!   credential store per request so rotation/revoke affects new requests,
//!   and the token's grant rides on the request to the core, which enforces
//!   it;
//! - every session ID belongs to the token name that initialized it, so
//!   another valid token cannot use its requests, streams, or deletion;
//! - rmcp checks the `Host` header (loopback names, plus the tunnel's
//!   `--allow-host`) and refuses any `Origin` that is not an
//!   `--allow-origin`, so a web page cannot drive it from a browser.
//!
//! Once bound, it takes the rho home's webhook watches (`watch/`) and
//! resumes them. Once `rho update` replaced its executable, it upgrades in
//! place (`app::http_upgrade`).
//!
//! The watches carry over an upgrade: they are on disk, and the new binary
//! resumes them, delivering any news that came meanwhile. A delivery under
//! way as the server re-executes may be sent again, as after a crash. A
//! host's MCP session does not carry over: the new binary knows none, so the
//! host starts a new one, as the protocol has it do for a session the server
//! no longer knows.

use std::{
    convert::Infallible, future::Future, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration,
};

use anyhow::Context;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rmcp::transport::{
    common::http_header::HEADER_SESSION_ID,
    streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService},
};

use super::{
    remotes::{Servers, CALL_TIMEOUT},
    token_store,
    tokens::TokenSet,
    watch::Watches,
    SessionTools, Transport,
};
use crate::app::{
    http_guard::{bearer_token, is_private, own_hosts, LOOPBACK_HOSTS},
    http_upgrade::{adopted_listener, Drain, InFlight, Upgrade},
    session_control::SessionControl,
};

/// The longest the MCP HTTP server waits for calls under way before
/// re-executing in place: one forwarded call (`CALL_TIMEOUT`) plus 20 s to
/// flush its answer.
const DRAIN_TIMEOUT: Duration = CALL_TIMEOUT.saturating_add(Duration::from_secs(20));

#[path = "http_sessions.rs"]
mod sessions;

use sessions::{Owner, Sessions};

/// The path the MCP endpoint answers on.
const ENDPOINT: &str = "/mcp";

/// How `rho mcp serve --http` listens.
pub(crate) struct HttpOptions {
    pub(crate) address: SocketAddr,
    /// `Host` values to accept besides loopback names, such as a tunnel's
    /// public hostname.
    pub(crate) allowed_hosts: Vec<String>,
    /// Browser origins to accept; requests with any other `Origin` are
    /// refused, and requests without one pass.
    pub(crate) allowed_origins: Vec<String>,
    /// The name webhook news says it is from (`--name`).
    pub(crate) name: String,
    /// How many sessions each token may watch at once (`--max-watches`).
    pub(crate) max_watches: u32,
    /// Where the watches are kept: `<rho home>/mcp-serve`.
    pub(crate) watches_dir: PathBuf,
    /// The listening socket a re-executing server handed this one
    /// (`app::http_upgrade`), on `address`, to serve on in place of binding it.
    pub(crate) inherited_listener: Option<std::net::TcpListener>,
    /// How the server upgrades in place once its executable is replaced;
    /// without one it keeps its binary.
    pub(crate) upgrade: Option<Upgrade>,
}

type Body = BoxBody<Bytes, Infallible>;
type Service = StreamableHttpService<SessionTools, Sessions>;

struct HttpServer {
    service: Service,
    sessions: Arc<Sessions>,
}

/// Serves the tools on `options.address` until the process is stopped,
/// the sessions they start running with `environment`.
pub(crate) async fn serve_http(
    control: Arc<SessionControl>,
    options: HttpOptions,
    environment: rho_protocol::ClientEnvironment,
) -> anyhow::Result<()> {
    let ip = options.address.ip();
    if !is_private(ip) {
        anyhow::bail!(
            "rho mcp serve --http binds a loopback address, such as 127.0.0.1:8765, or this machine's address on a private network, such as 192.168.1.20:8765 or a NetBird or Tailscale peer's 100.x.y.z:8765, not {}; reach it from elsewhere through a tunnel",
            options.address
        );
    }
    let own_hosts = own_hosts(ip);
    let listener = match options.inherited_listener {
        Some(socket) => adopted_listener(socket, options.address)?,
        None => tokio::net::TcpListener::bind(options.address)
            .await
            .with_context(|| format!("could not listen on {}", options.address))?,
    };
    let local = listener.local_addr()?;
    let servers = Servers::new(options.name.clone());
    let watches = Watches::open(
        Arc::clone(&control),
        options.name,
        options.max_watches,
        options.watches_dir,
    )?;
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(
            LOOPBACK_HOSTS
                .iter()
                .map(|host| (*host).to_owned())
                .chain(own_hosts.iter().cloned())
                .chain(options.allowed_hosts),
        )
        .with_allowed_origins(options.allowed_origins)
        .enforce_origin_validation();
    let sessions = Arc::new(Sessions::default());
    let service: Service = StreamableHttpService::new(
        move || {
            Ok(SessionTools {
                control: Arc::clone(&control),
                transport: Transport::Http {
                    watches: watches.clone(),
                },
                servers: servers.clone(),
                environment: environment.clone(),
            })
        },
        Arc::clone(&sessions),
        config,
    );
    let server = Arc::new(HttpServer { service, sessions });
    eprintln!("rho mcp serve listening on http://{local}{ENDPOINT}");
    if !own_hosts.is_empty() {
        eprintln!(
            "rho mcp serve: plain HTTP on a network address: anyone who can read traffic on that network can read requests, bearer tokens included"
        );
    }
    let drain = Drain::new();
    let Some(upgrade) = options.upgrade else {
        let never = std::future::pending::<String>();
        return Ok(accept_until(&listener, &server, &drain, never)
            .await
            .map(drop)?);
    };
    let version = accept_until(&listener, &server, &drain, upgrade.replaced()).await?;
    Err(drain
        .finish(upgrade, &version, listener, DRAIN_TIMEOUT)
        .await)
}

/// Serves each connection `listener` accepts until `replaced` resolves
/// with the version to upgrade to; connections that `drain` tells to
/// close close after the call they serve.
async fn accept_until(
    listener: &tokio::net::TcpListener,
    server: &Arc<HttpServer>,
    drain: &Drain,
    replaced: impl Future<Output = String>,
) -> std::io::Result<String> {
    tokio::pin!(replaced);
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => accepted?.0,
            version = &mut replaced => return Ok(version),
        };
        let server = Arc::clone(server);
        let calls = drain.calls();
        let began = drain.began();
        tokio::spawn(async move {
            let handler = hyper::service::service_fn(move |request| {
                let server = Arc::clone(&server);
                let calls = calls.clone();
                async move { Ok::<_, Infallible>(counted(&server, &calls, request).await) }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler);
            tokio::pin!(connection);
            let ended = tokio::select! {
                ended = connection.as_mut() => ended,
                () = began => {
                    connection.as_mut().graceful_shutdown();
                    connection.await
                }
            };
            if let Err(error) = ended {
                tracing::debug!(%error, "MCP HTTP connection ended");
            }
        });
    }
}

/// [`handle`], counting a POST, which carries a tool call, as under way
/// until its response's body is sent or dropped, so an upgrade waits for
/// it. A GET opens a host's event stream, which lasts as long as the host
/// keeps it, so an upgrade does not wait for those.
async fn counted(
    server: &HttpServer,
    in_flight: &InFlight,
    request: Request<Incoming>,
) -> Response<Body> {
    let call = (request.method() == hyper::Method::POST).then(|| in_flight.enter());
    let response = handle(server, request).await;
    match call {
        Some(call) => response.map(|body| {
            body.map_frame(move |frame| {
                // Held by the body, so dropped with it.
                let _call = &call;
                frame
            })
            .boxed()
        }),
        None => response,
    }
}

/// Authenticates `request` and hands it to rmcp with its token's grant.
async fn handle(server: &HttpServer, mut request: Request<Incoming>) -> Response<Body> {
    if request.uri().path() != ENDPOINT {
        return refusal(StatusCode::NOT_FOUND, "not found");
    }
    let Some(presented) = bearer_token(&request) else {
        return unauthorized("missing bearer token");
    };
    // Read past the OS store's per-process cache, so a rotation or revoke
    // another process saved applies to the next request.
    let tokens = match token_store::load_fresh::<TokenSet>().await {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "could not read the MCP server tokens");
            return refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not read the server's tokens",
            );
        }
    };
    let Some(token) = tokens.authenticate(&presented) else {
        return unauthorized("invalid bearer token");
    };
    let owner = Owner(token.name.clone());
    if request
        .headers()
        .get(HEADER_SESSION_ID)
        .is_some_and(|id| !server.sessions.owns(id, &owner))
    {
        // Do not disclose whether a session exists for a different principal.
        return refusal(StatusCode::NOT_FOUND, "not found");
    }
    request.extensions_mut().insert(owner);
    request.extensions_mut().insert(token.grant());
    server.service.handle(request).await
}

fn unauthorized(message: &str) -> Response<Body> {
    let mut response = refusal(StatusCode::UNAUTHORIZED, message);
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Bearer"),
    );
    response
}

fn refusal(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::from(message.to_owned())).boxed());
    *response.status_mut() = status;
    response
}
