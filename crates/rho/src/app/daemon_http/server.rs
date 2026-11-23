//! HTTP/1.1 + WebSocket server for `rho daemon serve --http`.

use std::{
    collections::BTreeMap, convert::Infallible, future::Future, net::SocketAddr, pin::Pin,
    sync::Arc, time::Duration,
};

use anyhow::Context;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{body::Incoming, header, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rho_protocol::ClientEnvironment;
use rho_sdk::CancellationToken;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    task::JoinSet,
};
use tokio_tungstenite::{
    tungstenite::{handshake::derive_accept_key, protocol::Role},
    WebSocketStream,
};

use super::{
    bridge::{relay_websocket, websocket_config},
    tokens::DaemonTokenSet,
};
use crate::app::{
    http_guard::{bearer_token, check_host, check_origin, is_private, own_hosts, LOOPBACK_HOSTS},
    http_upgrade::{adopted_listener, Upgrade},
    mcp_server::token_store,
};

/// WebSocket endpoint path served by `rho daemon serve --http`.
pub(super) const ENDPOINT: &str = "/ws";

/// How long an in-place upgrade waits for active WebSocket bridges to flush
/// closing frames and finish in-flight daemon responses before re-executing.
const UPGRADE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Options for `rho daemon serve --http`.
pub(crate) struct HttpOptions {
    pub(crate) address: SocketAddr,
    /// `Host` names to accept besides loopback and the bound private IP.
    pub(crate) allowed_hosts: Vec<String>,
    /// Browser `Origin` values to accept; requests with any other `Origin`
    /// are refused, and requests without one pass.
    pub(crate) allowed_origins: Vec<String>,
    /// The listening socket a re-executing server handed this one
    /// (`http_upgrade`), on `address`, to serve on in place of binding it.
    pub(crate) inherited_listener: Option<std::net::TcpListener>,
    /// How the server upgrades in place once its executable is replaced;
    /// without one it keeps its binary.
    pub(crate) upgrade: Option<Upgrade>,
}

type Body = BoxBody<Bytes, Infallible>;
type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// How the HTTP server opens a fresh raw stream to the local daemon for each
/// authenticated WebSocket connection.
pub(super) trait DaemonConnector: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn connect(&self) -> BoxFuture<anyhow::Result<Self::Stream>>;
}

/// How the HTTP server loads the active bearer token set on each upgrade.
pub(super) trait TokenLoader: Send + Sync + 'static {
    fn load(&self) -> BoxFuture<anyhow::Result<DaemonTokenSet>>;
}

/// Production loader: reads the credential store fresh on every request so
/// rotations and revocations apply immediately.
struct StoreTokenLoader;

impl TokenLoader for StoreTokenLoader {
    fn load(&self) -> BoxFuture<anyhow::Result<DaemonTokenSet>> {
        Box::pin(token_store::load_fresh::<DaemonTokenSet>())
    }
}

/// Production connector: connects raw to the local daemon, starting it first
/// if it is not yet running.
struct LocalDaemonConnector;

impl DaemonConnector for LocalDaemonConnector {
    type Stream = crate::daemon::RawStream;

    fn connect(&self) -> BoxFuture<anyhow::Result<Self::Stream>> {
        Box::pin(crate::app::daemon_cli::connect_raw_or_start())
    }
}

struct ServerState<C, T> {
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    host_env: Arc<BTreeMap<String, String>>,
    connector: C,
    tokens: T,
    shutdown: CancellationToken,
    bridges: Arc<tokio::sync::Mutex<JoinSet<()>>>,
}

/// How [`serve_with`] stopped accepting connections.
enum ServeEnd {
    /// Stopped by signal/caller shutdown.
    Stopped,
    /// Executable was replaced with `version`; the caller hands `listener` to
    /// the replacement.
    Upgrade {
        version: String,
        listener: TcpListener,
    },
}

/// Binds `options.address` (or adopts `options.inherited_listener`) and serves
/// `rho-protocol` over WebSocket (`GET /ws`) until `shutdown` resolves or the
/// executable is replaced and upgraded in place.
pub(crate) async fn serve_http(
    options: HttpOptions,
    environment: ClientEnvironment,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let ip = options.address.ip();
    if !is_private(ip) {
        anyhow::bail!(
            "rho daemon serve --http binds a loopback address, such as 127.0.0.1:8766, or this machine's address on a private network, such as 192.168.1.20:8766 or a NetBird or Tailscale peer's 100.x.y.z:8766, not {}; reach it from elsewhere through a tunnel",
            options.address
        );
    }
    let own = own_hosts(ip);
    let listener = match options.inherited_listener {
        Some(socket) => adopted_listener(socket, options.address)?,
        None => TcpListener::bind(options.address)
            .await
            .with_context(|| format!("could not listen on {}", options.address))?,
    };
    let local = listener.local_addr()?;
    eprintln!("rho daemon serve listening on ws://{local}{ENDPOINT}");
    if !own.is_empty() {
        eprintln!(
            "rho daemon serve: plain HTTP/WS on a network address: anyone who can read traffic on that network can read requests, bearer tokens included"
        );
    }
    let allowed_hosts: Vec<String> = LOOPBACK_HOSTS
        .iter()
        .map(|host| (*host).to_owned())
        .chain(own)
        .chain(options.allowed_hosts)
        .collect();
    let Some(upgrade) = options.upgrade else {
        serve_with(
            listener,
            allowed_hosts,
            options.allowed_origins,
            environment,
            LocalDaemonConnector,
            StoreTokenLoader,
            shutdown,
        )
        .await?;
        return Ok(());
    };
    match serve_until(
        listener,
        allowed_hosts,
        options.allowed_origins,
        environment,
        LocalDaemonConnector,
        StoreTokenLoader,
        shutdown,
        upgrade.replaced(),
    )
    .await?
    {
        ServeEnd::Stopped => Ok(()),
        ServeEnd::Upgrade { version, listener } => {
            eprintln!("rho daemon serve: upgrading to {version} in place");
            match listener.into_std() {
                Ok(listener) => Err(upgrade.relaunch(listener, &version)),
                Err(error) => {
                    Err(anyhow::Error::new(error).context("could not hand on the listening socket"))
                }
            }
        }
    }
}

/// Testable core accept loop with injectable daemon connector and token loader.
pub(super) async fn serve_with<C, T>(
    listener: TcpListener,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    environment: ClientEnvironment,
    connector: C,
    tokens: T,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()>
where
    C: DaemonConnector,
    T: TokenLoader,
{
    let _ = serve_until(
        listener,
        allowed_hosts,
        allowed_origins,
        environment,
        connector,
        tokens,
        shutdown,
        std::future::pending::<String>(),
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn serve_until<C, T>(
    listener: TcpListener,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    environment: ClientEnvironment,
    connector: C,
    tokens: T,
    shutdown: impl Future<Output = ()>,
    replaced: impl Future<Output = String>,
) -> anyhow::Result<ServeEnd>
where
    C: DaemonConnector,
    T: TokenLoader,
{
    let host_env: BTreeMap<String, String> = environment.into_iter().collect();
    let cancel = CancellationToken::new();
    let bridges = Arc::new(tokio::sync::Mutex::new(JoinSet::new()));
    let state = Arc::new(ServerState {
        allowed_hosts,
        allowed_origins,
        host_env: Arc::new(host_env),
        connector,
        tokens,
        shutdown: cancel.clone(),
        bridges: Arc::clone(&bridges),
    });
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);
    tokio::pin!(replaced);
    let end = loop {
        let stream = tokio::select! {
            biased;
            () = &mut shutdown => break ServeEnd::Stopped,
            version = &mut replaced => break ServeEnd::Upgrade { version, listener },
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(error) if is_transient_accept_error(&error) => {
                    tracing::debug!(%error, "daemon HTTP accept failed");
                    continue;
                }
                Err(error) => return Err(error).context("daemon HTTP accept failed"),
            }
        };
        let state = Arc::clone(&state);
        let cancel_conn = cancel.clone();
        connections.spawn(async move {
            let handler = hyper::service::service_fn(move |request| {
                let state = Arc::clone(&state);
                async move { Ok::<_, Infallible>(handle(state, request).await) }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .with_upgrades();
            tokio::pin!(connection);
            let ended = tokio::select! {
                ended = connection.as_mut() => ended,
                () = cancel_conn.cancelled() => {
                    connection.as_mut().graceful_shutdown();
                    connection.await
                }
            };
            if let Err(error) = ended {
                tracing::debug!(%error, "daemon HTTP connection ended");
            }
        });
    };
    cancel.cancel();
    // Drain active HTTP connections and WebSocket bridges (bounded on upgrade
    // so a stalled remote peer cannot block re-execution indefinitely).
    let drain = async {
        while connections.join_next().await.is_some() {}
        let mut bridges = bridges.lock().await;
        while bridges.join_next().await.is_some() {}
    };
    match &end {
        ServeEnd::Stopped => drain.await,
        ServeEnd::Upgrade { .. } => {
            if tokio::time::timeout(UPGRADE_DRAIN_TIMEOUT, drain)
                .await
                .is_err()
            {
                eprintln!(
                    "rho daemon serve: connections still active after {UPGRADE_DRAIN_TIMEOUT:?}; cutting them short to upgrade"
                );
            }
        }
    }
    Ok(end)
}

async fn handle<C: DaemonConnector, T: TokenLoader>(
    state: Arc<ServerState<C, T>>,
    request: Request<Incoming>,
) -> Response<Body> {
    if request.uri().path() != ENDPOINT {
        return refusal(StatusCode::NOT_FOUND, "not found");
    }
    if let Err(status) = check_host(&request, &state.allowed_hosts) {
        let message = if status == StatusCode::BAD_REQUEST {
            "missing or invalid Host header"
        } else {
            "Host header is not allowed"
        };
        return refusal(status, message);
    }
    if let Err(status) = check_origin(&request, &state.allowed_origins) {
        return refusal(status, "Origin header is not allowed");
    }
    let Some(presented) = bearer_token(&request) else {
        return unauthorized("missing bearer token");
    };
    let tokens = match state.tokens.load().await {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "could not read daemon HTTP tokens");
            return refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not read the server's tokens",
            );
        }
    };
    let Some(token) = tokens.authenticate(&presented) else {
        return unauthorized("invalid bearer token");
    };
    if request.method() != Method::GET {
        let mut response = refusal(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        response
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("GET"));
        return response;
    }
    let Some(ws_key) = validate_websocket_upgrade(&request) else {
        let mut response = refusal(
            StatusCode::UPGRADE_REQUIRED,
            "WebSocket upgrade required on /ws",
        );
        response.headers_mut().insert(
            header::UPGRADE,
            header::HeaderValue::from_static("websocket"),
        );
        response.headers_mut().insert(
            header::SEC_WEBSOCKET_VERSION,
            header::HeaderValue::from_static("13"),
        );
        return response;
    };
    let daemon_stream = match state.connector.connect().await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "could not reach local rho daemon");
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("could not reach the rho daemon: {error:#}"),
            );
        }
    };

    let accept = derive_accept_key(ws_key.as_bytes());
    let token_name = token.name.clone();
    let host_env = Arc::clone(&state.host_env);
    let shutdown = state.shutdown.clone();
    let upgrade = hyper::upgrade::on(request);
    {
        let mut bridges = state.bridges.lock().await;
        // Reap finished bridge tasks so the set stays bounded over long uptime.
        while bridges.try_join_next().is_some() {}
        bridges.spawn(async move {
            match upgrade.await {
                Ok(upgraded) => {
                    tracing::debug!(token = %token_name, "accepted daemon WebSocket client");
                    let ws = WebSocketStream::from_raw_socket(
                        TokioIo::new(upgraded),
                        Role::Server,
                        Some(websocket_config()),
                    )
                    .await;
                    relay_websocket(ws, daemon_stream, host_env, shutdown).await;
                    tracing::debug!(token = %token_name, "daemon WebSocket client disconnected");
                }
                Err(error) => {
                    tracing::debug!(token = %token_name, %error, "WebSocket upgrade failed");
                }
            }
        });
    }

    let mut response = Response::new(Full::new(Bytes::new()).boxed());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    response.headers_mut().insert(
        header::UPGRADE,
        header::HeaderValue::from_static("websocket"),
    );
    response.headers_mut().insert(
        header::CONNECTION,
        header::HeaderValue::from_static("Upgrade"),
    );
    if let Ok(value) = header::HeaderValue::from_str(&accept) {
        response
            .headers_mut()
            .insert(header::SEC_WEBSOCKET_ACCEPT, value);
    }
    response
}

/// Validates RFC 6455 client upgrade headers and returns `Sec-WebSocket-Key`.
fn validate_websocket_upgrade<B>(request: &Request<B>) -> Option<String> {
    let has_upgrade_ws = request
        .headers()
        .get_all(header::UPGRADE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("websocket"));
    let has_connection_upgrade = request
        .headers()
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
    let version_ok = request
        .headers()
        .get(header::SEC_WEBSOCKET_VERSION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|version| version.trim() == "13");
    let key = request
        .headers()
        .get(header::SEC_WEBSOCKET_KEY)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|key| !key.is_empty())?;
    (has_upgrade_ws && has_connection_upgrade && version_ok).then(|| key.to_owned())
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

fn is_transient_accept_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
    )
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
