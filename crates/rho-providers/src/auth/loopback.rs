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
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
    );
    stream.write_all(response.as_bytes()).await
}

async fn read_http_request(stream: &mut TcpStream) -> io::Result<String> {
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
mod tests {
    use super::*;

    #[tokio::test]
    async fn callback_url_uses_the_bound_ipv4_target() {
        let listener = bind_ipv4(0).await.unwrap();
        let url = callback_url(&listener, "/callback").unwrap();
        assert_eq!(
            url,
            format!(
                "http://127.0.0.1:{}/callback",
                listener.local_addr().unwrap().port()
            )
        );
    }

    #[tokio::test]
    async fn generated_callback_target_reaches_the_listener() {
        let listener = bind_ipv4(0).await.unwrap();
        let callback = url::Url::parse(&callback_url(&listener, "/callback").unwrap()).unwrap();
        let connection =
            TcpStream::connect((callback.host_str().unwrap(), callback.port().unwrap()))
                .await
                .unwrap();
        let (accepted, _) = listener.accept().await.unwrap();

        assert_eq!(
            connection.peer_addr().unwrap(),
            accepted.local_addr().unwrap()
        );
    }

    #[test]
    fn socket_address_format_supports_ipv6_callback_targets() {
        let address: std::net::SocketAddr = "[::1]:1234".parse().unwrap();
        assert_eq!(
            format!("http://{address}/callback"),
            "http://[::1]:1234/callback"
        );
    }
}
