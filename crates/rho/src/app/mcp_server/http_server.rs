//! `rho mcp serve --http`: the tools over rmcp's streamable HTTP service,
//! for hosts that reach MCP servers by URL through a tunnel.
//!
//! What this layer guards, in order:
//! - it binds a loopback address only, so the network reaches it only
//!   through a tunnel the user set up;
//! - every request carries a bearer token from `rho mcp token`, read from the
//!   credential store per request so a rotation or revoke applies at once,
//!   and the token's grant rides on the request to the core, which enforces
//!   it;
//! - rmcp checks the `Host` header (loopback names, plus the tunnel's
//!   `--allow-host`) and refuses any `Origin` that is not an
//!   `--allow-origin`, so a web page cannot drive it from a browser.

use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use anyhow::Context;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use super::{tokens::TokenSet, Grants, SessionTools};
use crate::{
    app::session_control::SessionControl,
    credential_store::{AppCredentialStore, UntrackedCredentialStore},
};

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
}

type Body = BoxBody<Bytes, Infallible>;
type Service = StreamableHttpService<SessionTools, LocalSessionManager>;

/// Serves the tools on `options.address` until the process is stopped.
pub(crate) async fn serve_http(
    control: Arc<SessionControl>,
    options: HttpOptions,
) -> anyhow::Result<()> {
    if !options.address.ip().is_loopback() {
        anyhow::bail!(
            "rho mcp serve --http binds a loopback address only, such as 127.0.0.1:8765, not {}; reach it from elsewhere through a tunnel",
            options.address
        );
    }
    let listener = tokio::net::TcpListener::bind(options.address)
        .await
        .with_context(|| format!("could not listen on {}", options.address))?;
    let local = listener.local_addr()?;
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(
            LOOPBACK_HOSTS
                .iter()
                .map(|host| (*host).to_owned())
                .chain(options.allowed_hosts),
        )
        .with_allowed_origins(options.allowed_origins)
        .enforce_origin_validation();
    let service: Service = StreamableHttpService::new(
        move || {
            Ok(SessionTools {
                control: Arc::clone(&control),
                grants: Grants::PerRequest,
            })
        },
        Arc::default(),
        config,
    );
    eprintln!("rho mcp serve listening on http://{local}{ENDPOINT}");
    loop {
        let (stream, _) = listener.accept().await?;
        let service = service.clone();
        tokio::spawn(async move {
            let handler = hyper::service::service_fn(move |request| {
                let service = service.clone();
                async move { Ok::<_, Infallible>(handle(&service, request).await) }
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

/// Authenticates `request` and hands it to rmcp with its token's grant.
async fn handle(service: &Service, mut request: Request<Incoming>) -> Response<Body> {
    if request.uri().path() != ENDPOINT {
        return refusal(StatusCode::NOT_FOUND, "not found");
    }
    let Some(presented) = bearer(&request) else {
        return unauthorized("missing bearer token");
    };
    let tokens = match load_tokens().await {
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
    request.extensions_mut().insert(token.grant());
    service.handle(request).await
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

/// The saved tokens, read past the OS store's per-process cache so a
/// rotation or revoke another process saved applies to the next request.
async fn load_tokens() -> anyhow::Result<TokenSet> {
    tokio::task::spawn_blocking(|| {
        AppCredentialStore::reread_external_logins();
        TokenSet::load(&UntrackedCredentialStore)
    })
    .await?
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
