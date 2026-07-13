use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn capture_form(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
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
            let Some((headers, received_body)) = text.split_once("\r\n\r\n") else {
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
            if received_body.len() >= content_length {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        request
    });
    (endpoint, handle)
}

#[test]
fn authorization_url_uses_registered_loopback_and_pkce() {
    let request =
        build_oauth_request_with_values("state".into(), "verifier".into(), "nonce".into());
    let url = Url::parse(&request.authorize_url).unwrap();
    let query = url
        .query_pairs()
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(url.as_str().split('?').next().unwrap(), AUTHORIZE_URL);
    assert_eq!(query.get("client_id").unwrap(), CLIENT_ID);
    assert_eq!(
        query.get("redirect_uri").unwrap(),
        "http://127.0.0.1:56121/callback"
    );
    assert_eq!(query.get("scope").unwrap(), SCOPE);
    assert_eq!(query.get("state").unwrap(), "state");
    assert_eq!(query.get("nonce").unwrap(), "nonce");
    assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
    assert_eq!(
        query.get("code_challenge").unwrap(),
        &pkce_challenge("verifier")
    );
}

#[tokio::test]
async fn code_exchange_echoes_xai_pkce_challenge() {
    let (endpoint, captured) =
        capture_form(r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#)
            .await;
    let request =
        build_oauth_request_with_values("state".into(), "verifier".into(), "nonce".into());

    let tokens = exchange_code_with_endpoint(
        &reqwest::Client::new(),
        "authorization-code",
        &request,
        &endpoint,
    )
    .await
    .unwrap();

    assert_eq!(tokens.access_token, "access");
    let captured = captured.await.unwrap();
    let body = captured.split_once("\r\n\r\n").unwrap().1;
    let form = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(form.get("grant_type").unwrap(), "authorization_code");
    assert_eq!(form.get("code").unwrap(), "authorization-code");
    assert_eq!(form.get("client_id").unwrap(), CLIENT_ID);
    assert_eq!(form.get("code_verifier").unwrap(), "verifier");
    assert_eq!(form.get("code_challenge").unwrap(), &request.challenge);
    assert_eq!(form.get("code_challenge_method").unwrap(), "S256");
}

#[tokio::test]
async fn device_setup_sends_client_and_offline_scopes() {
    let (endpoint, captured) = capture_form(
        r#"{"device_code":"device","user_code":"ABCD","verification_uri":"https://auth.x.ai/activate","expires_in":300,"interval":5}"#,
    )
    .await;

    let login = start_xai_device_login_with_endpoint(&reqwest::Client::new(), &endpoint)
        .await
        .unwrap();

    assert_eq!(login.user_code, "ABCD");
    let captured = captured.await.unwrap();
    let body = captured.split_once("\r\n\r\n").unwrap().1;
    let form = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(form.get("client_id").unwrap(), CLIENT_ID);
    assert_eq!(form.get("scope").unwrap(), SCOPE);
    assert!(form.get("scope").unwrap().contains("offline_access"));
}

#[test]
fn callback_requires_matching_state() {
    assert!(matches!(
        parse_callback_http_request("GET /callback?code=ok&state=state HTTP/1.1", "state"),
        CallbackParse::Outcome(CallbackOutcome::Code(code)) if code == "ok"
    ));
    assert!(matches!(
        parse_callback_http_request("GET /callback?code=ok&state=wrong HTTP/1.1", "state"),
        CallbackParse::Invalid(XaiOAuthError::InvalidCallback(_))
    ));
}

#[test]
fn callback_preserves_oauth_error_description() {
    assert!(matches!(
        parse_callback_http_request(
            "GET /callback?error=access_denied&error_description=not+allowed&state=state HTTP/1.1",
            "state"
        ),
        CallbackParse::Outcome(CallbackOutcome::Error(message)) if message == "not allowed"
    ));
}

#[test]
fn callback_accepts_form_post_body() {
    let request = "POST /callback HTTP/1.1\r\n\
content-type: application/x-www-form-urlencoded\r\n\
content-length: 27\r\n\
\r\n\
code=ok&state=state";
    match parse_callback_http_request(request, "state") {
        CallbackParse::Outcome(CallbackOutcome::Code(code)) => assert_eq!(code, "ok"),
        other => panic!("expected code outcome, got {other:?}"),
    }
}

#[test]
fn callback_ignores_probes_and_non_callback_paths() {
    assert!(matches!(
        parse_callback_http_request("", "state"),
        CallbackParse::Ignored
    ));
    assert!(matches!(
        parse_callback_http_request("GET /favicon.ico HTTP/1.1\r\n\r\n", "state"),
        CallbackParse::Ignored
    ));
    assert!(matches!(
        parse_callback_http_request("PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n", "state"),
        CallbackParse::Ignored
    ));
}

#[tokio::test]
async fn wait_for_callback_skips_probes_until_valid_get() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = "state".to_string();
    let waiter = tokio::spawn(async move { wait_for_callback(&listener, &state).await });

    // Empty probe connection — must not abort the flow.
    {
        let _probe = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Non-callback path probe.
    {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0_u8; 256];
        let _ = stream.read(&mut buf).await;
    }

    // Real OAuth redirect.
    {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /callback?code=ok&state=state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0_u8; 256];
        let len = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..len]);
        assert!(response.contains("login complete"), "{response}");
    }

    let outcome = waiter.await.unwrap().unwrap();
    assert_eq!(outcome, CallbackOutcome::Code("ok".into()));
}

#[test]
fn token_response_records_expiry() {
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let tokens = tokens_from_response(TokenResponse {
        access_token: Some("access".into()),
        refresh_token: Some("refresh".into()),
        id_token: Some("id".into()),
        expires_in: Some(60),
        error: None,
        error_description: None,
    })
    .unwrap();

    assert_eq!(tokens.access_token, "access");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
    assert!(tokens.expires_at_unix.unwrap() >= before + 60);
}
