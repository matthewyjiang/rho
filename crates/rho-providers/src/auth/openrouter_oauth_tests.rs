use super::*;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn capture_json_request(
    status: &'static str,
    response_body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&request);
            let Some((headers, body)) = text.split_once("\r\n\r\n") else {
                continue;
            };
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if body.len() >= content_length {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (endpoint, handle)
}

#[test]
fn authorization_url_uses_callback_and_s256_challenge() {
    let callback_url = "http://localhost:51423/callback";
    let request = build_oauth_request(callback_url, "verifier".into());
    let url = Url::parse(&request.authorize_url).unwrap();
    let query = url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(url.as_str().split('?').next().unwrap(), AUTHORIZE_URL);
    assert_eq!(query.len(), 3);
    assert_eq!(query.get("callback_url").unwrap(), callback_url);
    assert_eq!(
        query.get("code_challenge").unwrap(),
        &pkce_challenge("verifier")
    );
    assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
}

#[test]
fn pkce_challenge_matches_rfc_7636_s256_example() {
    assert_eq!(
        pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn generated_verifier_has_valid_length_and_characters() {
    let verifier = random_token(64);
    assert_eq!(verifier.len(), 64);
    assert!(verifier.bytes().all(|byte| byte.is_ascii_alphanumeric()));
}

#[test]
fn callback_parser_accepts_only_matching_get_callback_with_code() {
    let expected_path = "/callback/expected-nonce";
    assert!(matches!(
        parse_callback_http_request(
            "GET /callback/expected-nonce?code=authorization-code HTTP/1.1\r\nHost: localhost\r\n\r\n",
            expected_path,
        ),
        CallbackParse::Code(code) if code == "authorization-code"
    ));
    assert_eq!(
        parse_callback_http_request(
            "GET /callback/expected-nonce HTTP/1.1\r\n\r\n",
            expected_path,
        ),
        CallbackParse::Invalid
    );
    assert_eq!(
        parse_callback_http_request(
            "GET /callback/expected-nonce?code= HTTP/1.1\r\n\r\n",
            expected_path,
        ),
        CallbackParse::Invalid
    );
}

#[test]
fn callback_parser_ignores_unrelated_browser_probes_and_nonce_mismatches() {
    let expected_path = "/callback/expected-nonce";
    for probe in [
        "",
        "GET /favicon.ico HTTP/1.1\r\n\r\n",
        "GET /callback/wrong-nonce?code=unsolicited HTTP/1.1\r\n\r\n",
        "HEAD /callback/expected-nonce HTTP/1.1\r\n\r\n",
        "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
    ] {
        assert_eq!(
            parse_callback_http_request(probe, expected_path),
            CallbackParse::Ignored
        );
    }
}

#[test]
fn callback_parser_reports_denial_without_exposing_it_in_debug() {
    let callback = parse_callback_http_request(
        "GET /callback/nonce?error=access_denied&error_description=user+cancelled HTTP/1.1\r\n\r\n",
        "/callback/nonce",
    );
    assert_eq!(callback, CallbackParse::Denied("user cancelled".into()));
    assert_eq!(format!("{callback:?}"), "Denied([REDACTED])");
}

#[test]
fn secret_values_are_redacted_from_debug_output() {
    let request = build_oauth_request(
        "http://localhost:51423/callback",
        "pkce-verifier-secret".into(),
    );
    let request_debug = format!("{request:?}");
    assert!(!request_debug.contains("pkce-verifier-secret"));
    assert!(!request_debug.contains("code_challenge"));

    let callback = CallbackParse::Code("authorization-code-secret".into());
    assert!(!format!("{callback:?}").contains("authorization-code-secret"));
}

#[tokio::test]
async fn callback_waiter_skips_probes_and_sends_plain_text_success() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let waiter =
        tokio::spawn(async move { wait_for_callback(&listener, "/callback/expected-nonce").await });

    let mut probe = tokio::net::TcpStream::connect(address).await.unwrap();
    probe
        .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut probe_response = [0_u8; 256];
    let probe_len = probe.read(&mut probe_response).await.unwrap();
    assert!(String::from_utf8_lossy(&probe_response[..probe_len]).contains("404 Not Found"));

    let mut callback = tokio::net::TcpStream::connect(address).await.unwrap();
    callback
        .write_all(
            b"GET /callback/expected-nonce?code=oauth-code HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();
    let mut callback_response = [0_u8; 512];
    let callback_len = callback.read(&mut callback_response).await.unwrap();
    let callback_response = String::from_utf8_lossy(&callback_response[..callback_len]);
    assert!(callback_response.contains("200 OK"));
    assert!(callback_response.contains("content-type: text/plain"));
    assert!(callback_response.contains("Authorization received"));

    assert_eq!(waiter.await.unwrap().unwrap(), "oauth-code");
}

#[tokio::test]
async fn callback_waiter_returns_browser_denial_after_failure_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let waiter = tokio::spawn(async move { wait_for_callback(&listener, "/callback/nonce").await });

    let mut callback = tokio::net::TcpStream::connect(address).await.unwrap();
    callback
        .write_all(
            b"GET /callback/nonce?error=access_denied&error_description=user+cancelled HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = [0_u8; 256];
    let len = callback.read(&mut response).await.unwrap();
    assert!(String::from_utf8_lossy(&response[..len]).contains("400 Bad Request"));
    assert!(matches!(
        waiter.await.unwrap(),
        Err(OpenRouterOAuthError::OAuthDenied(message)) if message == "user cancelled"
    ));
}

#[tokio::test]
async fn key_exchange_posts_expected_json_and_returns_key() {
    let (endpoint, captured) = capture_json_request("200 OK", r#"{"key":"openrouter-key"}"#).await;

    let key = exchange_code_with_endpoint(
        &reqwest::Client::new(),
        "authorization-code",
        "pkce-verifier",
        &endpoint,
    )
    .await
    .unwrap();

    assert_eq!(key, "openrouter-key");
    let request = captured.await.unwrap();
    assert!(request.starts_with("POST / HTTP/1.1\r\n"));
    assert!(request
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
    let body = request.split_once("\r\n\r\n").unwrap().1;
    assert_eq!(
        serde_json::from_str::<Value>(body).unwrap(),
        serde_json::json!({
            "code": "authorization-code",
            "code_verifier": "pkce-verifier",
            "code_challenge_method": "S256"
        })
    );
}

#[tokio::test]
async fn key_exchange_reports_status_without_exposing_secrets() {
    let (endpoint, captured) =
        capture_json_request("403 Forbidden", r#"{"error":"invalid secret-code"}"#).await;

    let error = exchange_code_with_endpoint(
        &reqwest::Client::new(),
        "secret-code",
        "secret-verifier",
        &endpoint,
    )
    .await
    .unwrap_err();
    captured.await.unwrap();

    assert!(matches!(
        error,
        OpenRouterOAuthError::ExchangeStatus(reqwest::StatusCode::FORBIDDEN)
    ));
    let error_output = format!("{error:?} {error}");
    assert!(!error_output.contains("secret-code"));
    assert!(!error_output.contains("secret-verifier"));
}

#[tokio::test]
async fn key_exchange_rejects_missing_or_blank_key() {
    for body in [r#"{"other":"value"}"#, r#"{"key":"  "}"#] {
        let (endpoint, captured) = capture_json_request("200 OK", body).await;

        let error = exchange_code_with_endpoint(
            &reqwest::Client::new(),
            "authorization-code",
            "pkce-verifier",
            &endpoint,
        )
        .await
        .unwrap_err();
        captured.await.unwrap();

        assert!(matches!(error, OpenRouterOAuthError::MissingKey));
    }
}

#[tokio::test]
async fn key_exchange_rejects_invalid_json_without_exposing_body() {
    let (endpoint, captured) = capture_json_request("200 OK", "response-secret").await;

    let error = exchange_code_with_endpoint(
        &reqwest::Client::new(),
        "authorization-code",
        "pkce-verifier",
        &endpoint,
    )
    .await
    .unwrap_err();
    captured.await.unwrap();

    assert!(matches!(error, OpenRouterOAuthError::InvalidResponse(_)));
    let error_output = format!("{error:?} {error}");
    assert!(!error_output.contains("response-secret"));
}
