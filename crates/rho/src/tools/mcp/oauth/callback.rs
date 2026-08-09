//! Loopback redirect endpoint for the MCP authorization code flow.
//!
//! The listener binds an ephemeral port on 127.0.0.1 before the authorization
//! URL is built, so the redirect URI Rho registers is the one it is already
//! listening on. Only the exact callback path is answered; every other target
//! gets a 404 and the wait continues, so a stray browser request cannot end
//! the login or be turned into a redirect somewhere else.

use std::net::Ipv4Addr;

use anyhow::{bail, Context};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const CALLBACK_PATH: &str = "/oauth/callback";
/// A browser sends the request line immediately; this only bounds a peer that
/// connects and then stalls.
const REQUEST_READ_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const CHUNK_BYTES: usize = 2048;

const SUCCESS_BODY: &str = "Rho is authorized for this MCP server. You can close this tab.";
const FAILURE_BODY: &str = "Rho could not read this authorization response.";
const IGNORED_BODY: &str = "Not the Rho authorization callback.";

/// A bound loopback endpoint waiting for one authorization redirect.
pub(super) struct LoopbackRedirect {
    listener: TcpListener,
    redirect_uri: String,
}

impl LoopbackRedirect {
    pub(super) async fn bind() -> anyhow::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .context("could not bind the local MCP OAuth callback listener")?;
        // Format the bound address rather than `localhost`, which can resolve
        // to IPv6 in the browser while the listener holds an IPv4 socket.
        let address = listener.local_addr()?;
        Ok(Self {
            redirect_uri: format!("http://{address}{CALLBACK_PATH}"),
            listener,
        })
    }

    /// The exact redirect URI to register with the authorization server.
    pub(super) fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Wait for the authorization redirect and return its absolute URL.
    ///
    /// The caller hands that URL to rmcp, which matches `state` against the
    /// PKCE round it started and rejects anything else.
    pub(super) async fn wait_for_redirect(&self) -> anyhow::Result<String> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .context("MCP OAuth callback listener failed")?;
            let request = match read_request(&mut stream).await {
                Ok(request) => request,
                Err(error) => {
                    tracing::debug!(error = %error, "discarding unreadable MCP OAuth callback request");
                    respond(&mut stream, CallbackVerdict::Unreadable).await;
                    continue;
                }
            };
            match callback_target(&request) {
                Ok(target) => {
                    respond(&mut stream, CallbackVerdict::Accepted).await;
                    return Ok(format!("http://{}{target}", self.listener.local_addr()?));
                }
                Err(error) => {
                    tracing::debug!(error = %error, "ignoring non-callback request on the MCP OAuth listener");
                    respond(&mut stream, CallbackVerdict::NotOurs).await;
                }
            }
        }
    }
}

/// What the listener decided about one request, and therefore what it answers.
#[derive(Clone, Copy)]
enum CallbackVerdict {
    Accepted,
    Unreadable,
    NotOurs,
}

/// Extract the request target when the request is our callback.
///
/// Requires the exact callback path and a `state` parameter, so neither a
/// stray browser probe nor a crafted link without CSRF state can complete the
/// login.
pub(super) fn callback_target(request: &str) -> anyhow::Result<&str> {
    let request_line = request.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" {
        bail!("callback request used {method} rather than GET");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        bail!("callback request targeted `{path}` rather than `{CALLBACK_PATH}`");
    }
    if !query.split('&').any(|pair| {
        pair.split_once('=')
            .is_some_and(|(name, _)| name == "state")
    }) {
        bail!("callback request carried no `state` parameter");
    }
    Ok(target)
}

async fn respond(stream: &mut TcpStream, verdict: CallbackVerdict) {
    let (status, body) = match verdict {
        CallbackVerdict::Accepted => ("200 OK", SUCCESS_BODY),
        CallbackVerdict::Unreadable => ("400 Bad Request", FAILURE_BODY),
        CallbackVerdict::NotOurs => ("404 Not Found", IGNORED_BODY),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    if let Err(error) = stream.write_all(response.as_bytes()).await {
        tracing::debug!(error = %error, "could not answer the MCP OAuth callback request");
    }
}

/// Read until the end of the request headers. The body is irrelevant: the
/// authorization response travels in the query string.
async fn read_request(stream: &mut TcpStream) -> anyhow::Result<String> {
    let read = async {
        let mut request = Vec::new();
        let mut chunk = [0_u8; CHUNK_BYTES];
        loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            if request.len() > MAX_REQUEST_BYTES {
                bail!("callback request exceeded {MAX_REQUEST_BYTES} bytes");
            }
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&request).into_owned())
    };
    tokio::time::timeout(REQUEST_READ_BUDGET, read)
        .await
        .context("callback request stalled before its headers arrived")?
}

#[cfg(test)]
#[path = "callback_tests.rs"]
mod tests;
