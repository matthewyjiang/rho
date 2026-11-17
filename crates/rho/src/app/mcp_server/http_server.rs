//! `rho mcp serve --http`: the tools over rmcp's streamable HTTP service,
//! for hosts that reach MCP servers by URL through a tunnel.
//!
//! What this layer guards, in order:
//! - it binds a loopback address, which the network reaches only through a
//!   tunnel the user set up, or an address on a private network
//!   (`private_network.rs`), such as an overlay network's, where the owner
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
//! resumes them.

use std::{convert::Infallible, net::SocketAddr, path::PathBuf, sync::Arc};

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
    private_network::is_private, remotes::Servers, token_store, tokens::TokenSet, watch::Watches,
    SessionTools, Transport,
};
use crate::app::session_control::SessionControl;

#[path = "http_sessions.rs"]
mod sessions;

use sessions::{Owner, Sessions};

/// The path the MCP endpoint answers on.
const ENDPOINT: &str = "/mcp";

/// The `Host` names rmcp accepts by default, which `--allow-host` adds to.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

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
    let listener = tokio::net::TcpListener::bind(options.address)
        .await
        .with_context(|| format!("could not listen on {}", options.address))?;
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
    loop {
        let (stream, _) = listener.accept().await?;
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            let handler = hyper::service::service_fn(move |request| {
                let server = Arc::clone(&server);
                async move { Ok::<_, Infallible>(handle(&server, request).await) }
            });
            if let Err(error) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .await
            {
                tracing::debug!(%error, "MCP HTTP connection ended");
            }
        });
    }
}

/// The `Host`s requests to private address `ip` name, which are allowed
/// without `--allow-host`: none for loopback, which is always allowed. An
/// IPv6 address can be written two ways, as Rust prints it and as a URL
/// does, which writes a mapped IPv4 address in hex; both are allowed.
fn own_hosts(ip: std::net::IpAddr) -> Vec<String> {
    if ip.is_loopback() {
        return Vec::new();
    }
    let mut hosts = vec![ip.to_string()];
    if let std::net::IpAddr::V6(v6) = ip {
        let url_form = url::Host::<String>::Ipv6(v6).to_string();
        let url_form = url_form.trim_start_matches('[').trim_end_matches(']');
        if url_form != hosts[0] {
            hosts.push(url_form.to_owned());
        }
    }
    hosts
}

/// Authenticates `request` and hands it to rmcp with its token's grant.
async fn handle(server: &HttpServer, mut request: Request<Incoming>) -> Response<Body> {
    if request.uri().path() != ENDPOINT {
        return refusal(StatusCode::NOT_FOUND, "not found");
    }
    let Some(presented) = bearer(&request) else {
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

/// The token in an `Authorization: Bearer <token>` header.
fn bearer(request: &Request<Incoming>) -> Option<String> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim().to_owned())
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

#[cfg(test)]
#[path = "http_server_tests.rs"]
mod tests;
