use std::{io, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distributions::Alphanumeric, Rng};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

const MAX_REQUEST_SIZE: usize = 16 * 1024;
const CHUNK_SIZE: usize = 2048;

#[derive(Clone, Copy)]
pub(super) enum ResponseKind {
    Success,
    Failure,
    Ignored,
}

pub(super) fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

pub(super) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(super) async fn bind_ipv4(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await
}

/// Listener plus the callback URL derived from its bound address.
pub(super) struct BoundLoopback {
    pub listener: TcpListener,
    pub callback_url: String,
}

pub(super) enum LoopbackBindError {
    Bind(io::Error),
    LocalAddress(io::Error),
}

/// Bind a loopback port, then form the callback URL before the wait.
pub(super) async fn bind_loopback(
    port: u16,
    path: &str,
) -> Result<BoundLoopback, LoopbackBindError> {
    let listener = bind_ipv4(port).await.map_err(LoopbackBindError::Bind)?;
    let callback_url = callback_url(&listener, path).map_err(LoopbackBindError::LocalAddress)?;
    Ok(BoundLoopback {
        listener,
        callback_url,
    })
}

/// Builds a callback URL from the listener's bound address.
///
/// Using this exact address avoids binding IPv4 while asking the browser to
/// resolve `localhost`, which can select IPv6 on some systems.
pub(super) fn callback_url(listener: &TcpListener, path: &str) -> io::Result<String> {
    let address = listener.local_addr()?;
    Ok(format!("http://{address}{path}"))
}

pub(super) async fn accept_request(
    listener: &TcpListener,
    read_timeout: Duration,
) -> io::Result<(TcpStream, Option<String>)> {
    let (mut stream, _) = listener.accept().await?;
    let request = match timeout(read_timeout, read_http_request(&mut stream)).await {
        Ok(Ok(request)) if !request.trim().is_empty() => Some(request),
        _ => None,
    };
    Ok((stream, request))
}

#[derive(Clone, Copy)]
pub(super) struct ResponseBodies<'a> {
    pub success: &'a str,
    pub failure: &'a str,
    pub ignored: &'a str,
}

/// How the shared callback waiter should respond and return.
pub(super) enum CallbackDecision<T, E> {
    /// Write the success page and return `Ok(value)`.
    Success(T),
    /// Write the failure page and return `Ok(value)`.
    Failure(T),
    /// Write the failure page and return `Err(error)`.
    Invalid(E),
    /// Write the ignored page and keep waiting.
    Ignored,
}

pub(super) enum CallbackWaitError<E> {
    Accept(io::Error),
    Invalid(E),
}

/// Dual-stack loopback bind used by Codex (`127.0.0.1` and `::1` on one port).
pub(super) struct DualLoopback {
    ipv4: Option<TcpListener>,
    ipv6: Option<TcpListener>,
}

/// Bind IPv4 `127.0.0.1` and IPv6 `::1` on `port`. Succeeds if either stack binds.
///
/// When both fail, the IPv4 error is returned, matching the previous Codex binder.
pub(super) async fn bind_dual_loopback(port: u16) -> io::Result<DualLoopback> {
    let ipv4 = bind_ipv4(port).await;
    let ipv6 = bind_ipv6(port).await;
    match (ipv4, ipv6) {
        (Ok(ipv4), Ok(ipv6)) => Ok(DualLoopback {
            ipv4: Some(ipv4),
            ipv6: Some(ipv6),
        }),
        (Ok(ipv4), Err(_)) => Ok(DualLoopback {
            ipv4: Some(ipv4),
            ipv6: None,
        }),
        (Err(_), Ok(ipv6)) => Ok(DualLoopback {
            ipv4: None,
            ipv6: Some(ipv6),
        }),
        (Err(ipv4), Err(_)) => Err(ipv4),
    }
}

pub(super) async fn bind_ipv6(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)).await
}

/// Accept the first connection on either dual-stack listener.
pub(super) async fn accept_dual(listeners: &DualLoopback) -> io::Result<TcpStream> {
    match (&listeners.ipv4, &listeners.ipv6) {
        (Some(ipv4), Some(ipv6)) => {
            tokio::select! {
                result = ipv4.accept() => result,
                result = ipv6.accept() => result,
            }
        }
        (Some(ipv4), None) => ipv4.accept().await,
        (None, Some(ipv6)) => ipv6.accept().await,
        (None, None) => {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no OAuth callback listeners were available",
            ))
        }
    }
    .map(|(stream, _)| stream)
}

/// Wait until a callback parse finishes or fails. Probe requests stay ignored.
pub(super) async fn wait_for_oauth_callback<T, E>(
    listener: &TcpListener,
    parse: impl Fn(&str) -> CallbackDecision<T, E>,
    bodies: ResponseBodies<'_>,
    timeout: Duration,
) -> Result<T, CallbackWaitError<E>> {
    loop {
        let (mut stream, request) = accept_request(listener, timeout)
            .await
            .map_err(CallbackWaitError::Accept)?;
        let Some(request) = request else {
            let _ = write_response(&mut stream, ResponseKind::Ignored, bodies).await;
            continue;
        };
        match parse(&request) {
            CallbackDecision::Success(value) => {
                let _ = write_response(&mut stream, ResponseKind::Success, bodies).await;
                return Ok(value);
            }
            CallbackDecision::Failure(value) => {
                let _ = write_response(&mut stream, ResponseKind::Failure, bodies).await;
                return Ok(value);
            }
            CallbackDecision::Invalid(error) => {
                let _ = write_response(&mut stream, ResponseKind::Failure, bodies).await;
                return Err(CallbackWaitError::Invalid(error));
            }
            CallbackDecision::Ignored => {
                let _ = write_response(&mut stream, ResponseKind::Ignored, bodies).await;
            }
        }
    }
}

pub(super) async fn write_response(
    stream: &mut TcpStream,
    kind: ResponseKind,
    bodies: ResponseBodies<'_>,
) -> io::Result<()> {
    let (status, body) = match kind {
        ResponseKind::Success => ("200 OK", bodies.success),
        ResponseKind::Failure => ("400 Bad Request", bodies.failure),
        ResponseKind::Ignored => ("404 Not Found", bodies.ignored),
    };
    write_http_response(stream, status, body).await
}

pub(super) async fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
    );
    stream.write_all(response.as_bytes()).await
}

pub(super) async fn read_http_request(stream: &mut TcpStream) -> io::Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; CHUNK_SIZE];
    loop {
        let len = stream.read(&mut chunk).await?;
        if len == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..len]);
        if request.len() > MAX_REQUEST_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "OAuth callback request exceeded size limit",
            ));
        }
        if let Some(header_end) = find_header_end(&request) {
            let body_start = header_end + 4;
            let content_length = content_length(&request[..header_end]).unwrap_or(0);
            let total = body_start.checked_add(content_length).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid OAuth callback length")
            })?;
            if total > MAX_REQUEST_SIZE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "OAuth callback request exceeded size limit",
                ));
            }
            if request.len() >= total {
                break;
            }
        } else if request.windows(2).any(|window| window == b"\n\n") {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&request).into_owned())
}

fn find_header_end(request: &[u8]) -> Option<usize> {
    request.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &[u8]) -> Option<usize> {
    std::str::from_utf8(headers).ok()?.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

#[cfg(test)]
#[path = "loopback_tests.rs"]
mod tests;
