//! A one-request System One server for tests.

use std::time::Duration;

use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

/// The request a [`serve_once`] server received.
pub(crate) struct Received {
    pub request_line: String,
    pub authorization: Option<String>,
    pub body: Value,
}

/// Serves one HTTP request with `status` and `body` under the returned base,
/// `http://127.0.0.1:PORT/v1`.
pub(crate) async fn serve_once(
    status: u16,
    body: String,
) -> (Url, tokio::task::JoinHandle<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let header = |name: &str| {
                head.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case(name).then(|| value.trim().to_owned())
                })
            };
            let length = header("content-length").map_or(0, |value| value.parse().unwrap());
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            Received {
                request_line: head.lines().next().unwrap().to_owned(),
                authorization: header("authorization"),
                body: serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
            }
        })
        .await
        .expect("mock System One request timed out")
    });
    (base, task)
}
