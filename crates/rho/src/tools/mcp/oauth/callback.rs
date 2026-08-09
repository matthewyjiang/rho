//! Loopback redirect endpoint for the MCP authorization code flow.
//!
//! The listener binds an ephemeral port on 127.0.0.1 before the authorization
//! URL is built, so the redirect URI Rho registers is the one it is already
//! listening on. Only the exact callback path with a matching CSRF `state` is
//! answered as success; every other target gets a non-success response and the
//! wait continues, so a stray browser request or a crafted link with the wrong
//! state cannot end the login or pre-empt the real browser callback.

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
const STATE_MISMATCH_BODY: &str =
    "This authorization response did not match the pending Rho login. Waiting for the correct one.";

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

    /// Wait for the authorization redirect whose `state` matches `expected_state`.
    ///
    /// RFC 8252 §8.9 requires rejecting responses whose state does not match
    /// the pending request. Mismatched or absent state must not terminate the
    /// wait or receive a success page; the matching browser callback must still
    /// be accepted afterward.
    pub(super) async fn wait_for_redirect(&self, expected_state: &str) -> anyhow::Result<String> {
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
            match callback_target(&request, expected_state) {
                Ok(target) => {
                    respond(&mut stream, CallbackVerdict::Accepted).await;
                    return Ok(format!("http://{}{target}", self.listener.local_addr()?));
                }
                Err(CallbackReject::WrongState) => {
                    tracing::debug!(
                        "ignoring MCP OAuth callback with a non-matching state parameter"
                    );
                    respond(&mut stream, CallbackVerdict::WrongState).await;
                }
                Err(CallbackReject::NotOurs(error)) => {
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
    WrongState,
}

/// Why a request was not accepted as the pending callback.
#[derive(Debug)]
pub(super) enum CallbackReject {
    WrongState,
    NotOurs(anyhow::Error),
}

/// Extract the request target when the request is our callback for this round.
///
/// Requires the exact callback path and a `state` parameter equal to the CSRF
/// token Rho put in the authorization URL, so neither a stray browser probe nor
/// a crafted link with a different state can complete the login.
pub(super) fn callback_target<'a>(
    request: &'a str,
    expected_state: &str,
) -> Result<&'a str, CallbackReject> {
    let request_line = request.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" {
        return Err(CallbackReject::NotOurs(anyhow::anyhow!(
            "callback request used {method} rather than GET"
        )));
    }
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    if path != CALLBACK_PATH {
        return Err(CallbackReject::NotOurs(anyhow::anyhow!(
            "callback request targeted `{path}` rather than `{CALLBACK_PATH}`"
        )));
    }
    // Match rmcp's AuthorizationCallback::from_redirect_url: decoded
    // application/x-www-form-urlencoded pairs, last-wins for duplicate keys.
    // The listener must accept exactly the state value that handle_callback_url
    // will validate after this request is turned into a full redirect URL.
    let callback_url = format!("http://127.0.0.1{target}");
    let url = url::Url::parse(&callback_url).map_err(|error| {
        CallbackReject::NotOurs(anyhow::anyhow!(
            "callback target was not a valid URL: {error}"
        ))
    })?;
    let mut state = None;
    for (name, value) in url.query_pairs() {
        if name == "state" {
            state = Some(value);
        }
    }
    match state.as_deref() {
        None => Err(CallbackReject::NotOurs(anyhow::anyhow!(
            "callback request carried no `state` parameter"
        ))),
        Some(value) if value == expected_state => Ok(target),
        Some(_) => Err(CallbackReject::WrongState),
    }
}

/// Pull the CSRF `state` parameter out of the authorization URL rmcp built.
pub(super) fn state_from_authorization_url(auth_url: &str) -> anyhow::Result<String> {
    let url = url::Url::parse(auth_url).context("authorization URL was not a valid URL")?;
    url.query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.into_owned())
        .context("authorization URL carried no `state` parameter")
}

async fn respond(stream: &mut TcpStream, verdict: CallbackVerdict) {
    let (status, body) = match verdict {
        CallbackVerdict::Accepted => ("200 OK", SUCCESS_BODY),
        CallbackVerdict::Unreadable => ("400 Bad Request", FAILURE_BODY),
        CallbackVerdict::NotOurs => ("404 Not Found", IGNORED_BODY),
        CallbackVerdict::WrongState => ("400 Bad Request", STATE_MISMATCH_BODY),
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
