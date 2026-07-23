use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

use super::super::{
    auth::{Auth, CodexAuthSource},
    codex_request::{build_responses_compact_body, build_responses_create_body, ResponsesProfile},
    reasoning::OpenAiReasoningProfile,
};
use super::{ResponsesEndpoint, ResponsesFailedAttemptKind, ResponsesHttpTransport};
use crate::{
    credentials::{CodexTokens, MemoryCredentialStore},
    model::{Message, ModelError, ModelRequest},
};

#[derive(Clone, Default)]
struct CapturedRequest {
    path: String,
    headers: String,
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut buf = vec![0; 16_384];
    let mut request = Vec::new();
    loop {
        let bytes = stream.read(&mut buf).await.unwrap();
        if bytes == 0 {
            break;
        }
        request.extend_from_slice(&buf[..bytes]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = &request[..header_end + 4];
        let headers_text = String::from_utf8_lossy(headers);
        let content_length = headers_text
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

type TestResponse = Box<dyn Fn(&CapturedRequest) -> (u16, String) + Send>;

async fn spawn_sequential_server(
    responses: Vec<TestResponse>,
) -> (String, Arc<Mutex<Vec<CapturedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let server_captured = Arc::clone(&captured);
    tokio::spawn(async move {
        let mut remaining = responses.into_iter();
        while let Ok((mut stream, _)) = listener.accept().await {
            let raw = read_http_request(&mut stream).await;
            let raw = String::from_utf8_lossy(&raw);
            let (headers, _body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_ref(), ""));
            let path = headers
                .lines()
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let captured_request = CapturedRequest {
                path,
                headers: headers.to_string(),
            };
            server_captured.lock().await.push(captured_request.clone());
            let Some(respond) = remaining.next() else {
                let _ = stream
                    .write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n")
                    .await;
                continue;
            };
            let (status, body) = respond(&captured_request);
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                _ => "Error",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (base, captured)
}

fn transport<'a>(
    client: &'a reqwest::Client,
    api_base: &'a str,
    profile: &'a ResponsesProfile,
    store: &'a MemoryCredentialStore,
    refreshed: &'a std::sync::Mutex<Option<CodexTokens>>,
) -> ResponsesHttpTransport<'a> {
    ResponsesHttpTransport::new(client, api_base, profile, store, refreshed)
}

#[tokio::test]
async fn api_key_create_and_compact_send_expected_headers_and_paths() {
    let (base, captured) = spawn_sequential_server(vec![
        Box::new(|_| (200, r#"{"ok":true}"#.into())),
        Box::new(|_| (200, r#"{"ok":true}"#.into())),
    ])
    .await;
    let client = reqwest::Client::new();
    let store = MemoryCredentialStore::default();
    let refreshed = std::sync::Mutex::new(None);
    let profile = ResponsesProfile::from_auth(&Auth::ApiKey("sk-test".into()), "gpt-5.4");
    let http = transport(&client, &base, &profile, &store, &refreshed);
    let body = json!({"model":"gpt-5.4","store":false});

    let create = http
        .post_json(
            &Auth::ApiKey("sk-test".into()),
            ResponsesEndpoint::Create,
            &body,
            None,
        )
        .await;
    assert!(create.failed_attempts.is_empty());
    assert_eq!(create.response.unwrap().status(), reqwest::StatusCode::OK);

    let compact = http
        .post_json(
            &Auth::ApiKey("sk-test".into()),
            ResponsesEndpoint::Compact,
            &body,
            None,
        )
        .await;
    assert!(compact.failed_attempts.is_empty());
    assert_eq!(compact.response.unwrap().status(), reqwest::StatusCode::OK);

    let requests = captured.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/responses");
    let create_headers = requests[0].headers.to_ascii_lowercase();
    assert!(create_headers.contains("authorization: bearer sk-test"));
    assert!(create_headers.contains("user-agent: rho"));
    assert!(!create_headers.contains("openai-beta"));

    assert_eq!(requests[1].path, "/responses/compact");
    let compact_headers = requests[1].headers.to_ascii_lowercase();
    assert!(compact_headers.contains("user-agent: rho"));
}

#[tokio::test]
async fn codex_create_and_compact_send_expected_headers_and_paths() {
    let (base, captured) = spawn_sequential_server(vec![
        Box::new(|_| (200, r#"{"ok":true}"#.into())),
        Box::new(|_| (200, r#"{"ok":true}"#.into())),
    ])
    .await;
    let client = reqwest::Client::new();
    let store = MemoryCredentialStore::default();
    let refreshed = std::sync::Mutex::new(None);
    let auth = Auth::Codex {
        tokens: CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: Some("acct_1".into()),
        },
        source: CodexAuthSource::Env,
    };
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let http = transport(&client, &base, &profile, &store, &refreshed);
    let body = json!({"model":"gpt-5.4","store":false});

    let create = http
        .post_json(&auth, ResponsesEndpoint::Create, &body, None)
        .await;
    assert!(create.failed_attempts.is_empty());
    assert!(create.response.is_ok());

    let compact = http
        .post_json(&auth, ResponsesEndpoint::Compact, &body, None)
        .await;
    assert!(compact.failed_attempts.is_empty());
    assert!(compact.response.is_ok());

    let requests = captured.lock().await.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/responses");
    let create_headers = requests[0].headers.to_ascii_lowercase();
    assert!(create_headers.contains("authorization: bearer access"));
    assert!(create_headers.contains("user-agent: codex-cli"));
    assert!(create_headers.contains("originator: codex_cli_rs"));
    assert!(create_headers.contains("chatgpt-account-id: acct_1"));
    assert!(!create_headers.contains("openai-beta"));

    assert_eq!(requests[1].path, "/responses/compact");
    let compact_headers = requests[1].headers.to_ascii_lowercase();
    assert!(compact_headers.contains("openai-beta: responses=experimental"));
    assert!(compact_headers.contains("user-agent: codex-cli"));
}

#[tokio::test]
async fn codex_compact_401_refresh_reports_auth_failed_attempt_and_retries() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let server_captured = Arc::clone(&captured);
    let compact_hits = Arc::new(AtomicUsize::new(0));
    let server_compact_hits = Arc::clone(&compact_hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let raw = read_http_request(&mut stream).await;
            let raw = String::from_utf8_lossy(&raw);
            let (headers, _body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_ref(), ""));
            let path = headers
                .lines()
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            server_captured.lock().await.push(CapturedRequest {
                path: path.clone(),
                headers: headers.to_string(),
            });
            let response = if path.contains("oauth/token") {
                let body = r#"{"access_token":"access-2","refresh_token":"refresh-2"}"#;
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
            } else {
                let n = server_compact_hits.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n".into()
                } else {
                    let body = r#"{"ok":true}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                        body.len()
                    )
                }
            };
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });

    let client = reqwest::Client::new();
    let store = MemoryCredentialStore::default();
    let refreshed = std::sync::Mutex::new(None);
    let auth = Auth::Codex {
        tokens: CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: Some("acct_1".into()),
        },
        source: CodexAuthSource::Env,
    };
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let refresh_url = format!("{base}/oauth/token");
    let http = transport(&client, &base, &profile, &store, &refreshed)
        .with_codex_refresh_url(&refresh_url);

    let result = http
        .post_json(
            &auth,
            ResponsesEndpoint::Compact,
            &json!({"model":"gpt-5.4","store":false}),
            None,
        )
        .await;
    assert_eq!(result.response.unwrap().status(), reqwest::StatusCode::OK);
    assert_eq!(result.failed_attempts.len(), 1);
    assert_eq!(
        result.failed_attempts[0].kind,
        ResponsesFailedAttemptKind::Authentication
    );

    let requests = captured.lock().await.clone();
    assert!(requests
        .iter()
        .any(|request| request.path == "/responses/compact"));
    assert!(requests
        .iter()
        .any(|request| request.path.contains("oauth/token")));
    let retry = requests
        .iter()
        .filter(|request| request.path == "/responses/compact")
        .nth(1)
        .expect("retried compact request");
    assert!(retry
        .headers
        .to_ascii_lowercase()
        .contains("authorization: bearer access-2"));
    assert!(retry
        .headers
        .to_ascii_lowercase()
        .contains("openai-beta: responses=experimental"));
}

#[tokio::test]
async fn cancellation_during_send_returns_interrupted() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(tokio::sync::Notify::new());
    let server_accepted = Arc::clone(&accepted);
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_http_request(&mut stream).await;
        server_accepted.notify_one();
        tokio::time::sleep(Duration::from_secs(30)).await;
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
            .await;
    });

    let client = reqwest::Client::new();
    let store = MemoryCredentialStore::default();
    let refreshed = std::sync::Mutex::new(None);
    let auth = Auth::ApiKey("sk-test".into());
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let http = transport(&client, &base, &profile, &store, &refreshed);
    let cancellation = rho_sdk::CancellationToken::new();
    let cancel = cancellation.clone();
    let body = json!({"model":"gpt-5.4"});
    let post = http.post_json(&auth, ResponsesEndpoint::Create, &body, Some(&cancellation));
    let cancel_task = async move {
        accepted.notified().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(post, cancel_task);
    assert!(matches!(result.response, Err(ModelError::Interrupted)));
    assert!(result.failed_attempts.is_empty());
}

#[tokio::test]
async fn cancellation_during_refresh_returns_interrupted() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let refresh_started = Arc::new(tokio::sync::Notify::new());
    let server_refresh_started = Arc::clone(&refresh_started);
    tokio::spawn(async move {
        // First compact => 401.
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_http_request(&mut stream).await;
        let _ = stream
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await;
        let _ = stream.shutdown().await;

        // Refresh request hangs until cancelled.
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_http_request(&mut stream).await;
        server_refresh_started.notify_one();
        // Keep the socket open until the client drops the refresh future.
        let mut buf = [0u8; 64];
        let _ = stream.read(&mut buf).await;
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let store = MemoryCredentialStore::default();
    let refreshed = std::sync::Mutex::new(None);
    let auth = Auth::Codex {
        tokens: CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: None,
        },
        source: CodexAuthSource::Env,
    };
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let refresh_url = format!("{base}/oauth/token");
    let http = transport(&client, &base, &profile, &store, &refreshed)
        .with_codex_refresh_url(&refresh_url);
    let cancellation = rho_sdk::CancellationToken::new();
    let cancel = cancellation.clone();
    let body = json!({"model":"gpt-5.4"});
    let post = http.post_json(
        &auth,
        ResponsesEndpoint::Compact,
        &body,
        Some(&cancellation),
    );
    let cancel_task = async move {
        refresh_started.notified().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(post, cancel_task);
    assert!(matches!(result.response, Err(ModelError::Interrupted)));
    assert_eq!(result.failed_attempts.len(), 1);
    assert_eq!(
        result.failed_attempts[0].kind,
        ResponsesFailedAttemptKind::Authentication
    );
}

#[tokio::test]
async fn refresh_failure_retains_authentication_failed_attempt() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        // First compact => 401.
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_http_request(&mut stream).await;
        let _ = stream
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await;
        let _ = stream.shutdown().await;

        // Refresh fails permanently.
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_http_request(&mut stream).await;
        let _ = stream
            .write_all(
                b"HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
            )
            .await;
        let _ = stream.shutdown().await;
    });

    let client = reqwest::Client::new();
    let store = MemoryCredentialStore::default();
    let refreshed = std::sync::Mutex::new(None);
    let auth = Auth::Codex {
        tokens: CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: None,
        },
        source: CodexAuthSource::Env,
    };
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let refresh_url = format!("{base}/oauth/token");
    let http = transport(&client, &base, &profile, &store, &refreshed)
        .with_codex_refresh_url(&refresh_url);

    let result = http
        .post_json(
            &auth,
            ResponsesEndpoint::Compact,
            &json!({"model":"gpt-5.4"}),
            None,
        )
        .await;
    assert!(result.response.is_err());
    assert_eq!(result.failed_attempts.len(), 1);
    assert_eq!(
        result.failed_attempts[0].kind,
        ResponsesFailedAttemptKind::Authentication
    );
}

#[tokio::test]
async fn retry_send_failure_retains_authentication_failed_attempt() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let compact_hits = Arc::new(AtomicUsize::new(0));
    let server_compact_hits = Arc::clone(&compact_hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let raw = read_http_request(&mut stream).await;
            let raw = String::from_utf8_lossy(&raw);
            let (headers, _body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_ref(), ""));
            let path = headers
                .lines()
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            let response = if path.contains("oauth/token") {
                let body = r#"{"access_token":"access-2","refresh_token":"refresh-2"}"#;
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                let n = server_compact_hits.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .into()
                } else {
                    // Drop the connection so the retry send fails before a response.
                    let _ = stream.shutdown().await;
                    continue;
                }
            };
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    let client = reqwest::Client::new();
    let store = MemoryCredentialStore::default();
    let refreshed = std::sync::Mutex::new(None);
    let auth = Auth::Codex {
        tokens: CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: None,
        },
        source: CodexAuthSource::Env,
    };
    let profile = ResponsesProfile::from_auth(&auth, "gpt-5.4");
    let refresh_url = format!("{base}/oauth/token");
    let http = transport(&client, &base, &profile, &store, &refreshed)
        .with_codex_refresh_url(&refresh_url);

    let result = http
        .post_json(
            &auth,
            ResponsesEndpoint::Compact,
            &json!({"model":"gpt-5.4"}),
            None,
        )
        .await;
    assert!(result.response.is_err());
    assert_eq!(result.failed_attempts.len(), 1);
    assert_eq!(
        result.failed_attempts[0].kind,
        ResponsesFailedAttemptKind::Authentication
    );
}

#[test]
fn create_and_compact_body_builders_diverge_on_tools() {
    let profile = ResponsesProfile::from_auth(&Auth::ApiKey("key".into()), "gpt-5.4");
    let request = ModelRequest {
        messages: &[Message::user_text("hello")],
        tools: &[rho_tools::tool::ToolSpec {
            name: "bash".into(),
            description: "run".into(),
            input_schema: json!({"type":"object"}),
        }],
        cancellation: Default::default(),
        reasoning_level: Default::default(),
        prompt_cache_key: None,
    };
    let create = build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request.clone(),
    )
    .unwrap();
    let compact =
        build_responses_compact_body(&profile, &OpenAiReasoningProfile::unknown(), request)
            .unwrap();
    assert_eq!(create["stream"], true);
    assert!(create.get("tools").is_some());
    assert!(compact.get("stream").is_none());
    assert!(compact.get("tools").is_none());
    assert!(compact.get("tool_choice").is_none());
    assert!(compact.get("parallel_tool_calls").is_none());
}
