use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn device_authorization_posts_form_client_id_and_parses_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/api/oauth/device_authorization",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("POST /api/oauth/device_authorization HTTP/1.1"));
        assert!(request.contains("application/x-www-form-urlencoded"));
        assert!(request.contains(&format!("client_id={CLIENT_ID}")));
        let body = r#"{"device_code":"device-secret","user_code":"ABCD-EFGH","verification_uri":"https://auth.kimi.com/device","verification_uri_complete":"https://auth.kimi.com/device?code=secret","expires_in":300,"interval":5}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let login = start_with_endpoint(&reqwest::Client::new(), &endpoint)
        .await
        .unwrap();
    assert_eq!(login.user_code, "ABCD-EFGH");
    assert_eq!(login.verification_uri, "https://auth.kimi.com/device");
    server.await.unwrap();
}

#[tokio::test]
async fn refresh_posts_form_and_returns_rotated_tokens() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api/oauth/token", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.starts_with("POST /api/oauth/token HTTP/1.1"));
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=old-refresh"));
        assert!(request.contains(&format!("client_id={CLIENT_ID}")));
        let body = r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600,"scope":"kimi:code","token_type":"Bearer"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let tokens =
        refresh_kimi_tokens_with_endpoint(&reqwest::Client::new(), "old-refresh", &endpoint)
            .await
            .unwrap();
    assert_eq!(tokens.access_token, "new-access");
    assert_eq!(tokens.refresh_token.as_deref(), Some("new-refresh"));
    assert_eq!(tokens.scope, "kimi:code");
    assert_eq!(tokens.token_type, "Bearer");
    assert_eq!(tokens.expires_in, Some(3_600));
    assert!(tokens
        .expires_at_unix
        .is_some_and(|expires| expires > now_unix()));
    server.await.unwrap();
}

#[tokio::test]
async fn refresh_retries_rate_limits_and_accepts_rotated_tokens() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api/oauth/token", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for (status, body) in [
            (
                "429 Too Many Requests",
                r#"{"error":"temporarily_unavailable"}"#,
            ),
            (
                "200 OK",
                r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#,
            ),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let bytes = stream.read(&mut request).await.unwrap();
            assert!(bytes > 0);
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let tokens =
        refresh_kimi_tokens_with_endpoint(&reqwest::Client::new(), "old-refresh", &endpoint)
            .await
            .unwrap();

    assert_eq!(tokens.access_token, "new-access");
    server.await.unwrap();
}

#[test]
fn debug_redacts_device_codes_and_complete_uri() {
    let login = KimiDeviceLogin {
        user_code: "user-secret".into(),
        verification_uri: "https://auth.kimi.com/device".into(),
        verification_uri_complete: Some("https://auth.kimi.com/device?secret".into()),
        device_code: "device-secret".into(),
        expires_in: Duration::from_secs(300),
        interval: Duration::from_secs(5),
    };
    let debug = format!("{login:?}");
    assert!(!debug.contains("user-secret"));
    assert!(!debug.contains("device-secret"));
    assert!(!debug.contains("?secret"));
}
