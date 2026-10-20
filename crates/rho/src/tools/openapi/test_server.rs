//! A local HTTP/1.1 server for OpenAPI tests. It records every request and
//! answers each from a handler, one request per connection.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

/// One request as the server saw it. Header names are lowercase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Recorded {
    pub(super) method: String,
    /// Path and query, as sent.
    pub(super) target: String,
    pub(super) headers: BTreeMap<String, String>,
}

pub(super) struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    chunked: bool,
}

impl Reply {
    pub(super) fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            chunked: false,
        }
    }

    pub(super) fn json(status: u16, body: &serde_json::Value) -> Self {
        Self::new(status, body.to_string()).header("content-type", "application/json")
    }

    pub(super) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Send the body chunked, without a `Content-Length`.
    pub(super) fn chunked(mut self) -> Self {
        self.chunked = true;
        self
    }
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

pub(super) struct TestServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    task: JoinHandle<()>,
}

impl TestServer {
    pub(super) async fn start(
        handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, handler.clone(), recorded.clone()));
            }
        });
        Self {
            addr,
            requests,
            task,
        }
    }

    /// `http://127.0.0.1:<port><path>`.
    pub(super) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub(super) fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(stream: TcpStream, handler: Arc<Handler>, requests: Arc<Mutex<Vec<Recorded>>>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|length| length.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).await.is_err() {
        return;
    }
    let request = Recorded {
        method,
        target,
        headers,
    };
    let reply = handler(&request);
    requests.lock().unwrap().push(request);

    let mut head = format!("HTTP/1.1 {} Test\r\nconnection: close\r\n", reply.status);
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let mut out = Vec::new();
    if reply.chunked {
        head.push_str("transfer-encoding: chunked\r\n\r\n");
        out.extend_from_slice(head.as_bytes());
        for chunk in reply.body.chunks(64 * 1024) {
            out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            out.extend_from_slice(chunk);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n\r\n");
    } else {
        head.push_str(&format!("content-length: {}\r\n\r\n", reply.body.len()));
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&reply.body);
    }
    let mut stream = reader.into_inner();
    let _ = stream.write_all(&out).await;
    let _ = stream.shutdown().await;
}

/// Load the source `[openapi.test]` from `source` (its TOML table body), with
/// `spec` written to `spec.json` beside it and `env` as the environment.
pub(super) async fn load_test_source(
    spec: &serde_json::Value,
    source: &str,
    env: &[(&str, &str)],
    max_response_bytes: usize,
) -> super::bundle::OpenApiLoadOutcome {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("spec.json"), spec.to_string()).unwrap();
    let user: super::config::OpenApiConfig =
        toml::from_str(&format!("[test]\nspec = \"spec.json\"\n{source}")).unwrap();
    let sources = super::config::OpenApiSources {
        user,
        user_dir: dir.path().to_path_buf(),
        project: None,
    };
    let env: BTreeMap<String, String> = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let plans = sources.plan(&|name| env.get(name).cloned());
    super::bundle::load(
        plans,
        super::bundle::LoadOptions {
            cache: None,
            max_output_bytes: 64 * 1024,
            max_response_bytes,
        },
    )
    .await
}
