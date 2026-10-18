use super::*;
use crate::credentials::MemoryCredentialStore;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

/// Answers every request to `/<status>/<code>` with that status and OAuth
/// error code.
async fn token_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let route = line.split_whitespace().nth(1).unwrap().to_owned();
                let (status, code) = route[1..].split_once('/').unwrap();
                let (status, code) = (status.to_owned(), code.to_owned());
                let mut length = 0;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                reader.read_exact(&mut vec![0; length]).await.unwrap();
                let body = serde_json::json!({ "error": code }).to_string();
                let reply = format!(
                    "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = reader.get_mut().write_all(reply.as_bytes()).await;
            });
        }
    });
    base
}

// Covers: a refresh token the endpoint refuses (expired or revoked) must read
// as missing credentials, the error hosts answer with a sign-in prompt, while
// a malformed request, an endpoint outage, or a proxy block stays an HTTP
// failure.
// Owner: Codex OAuth refresh, over real HTTP.
#[tokio::test]
async fn refused_refresh_grants_report_missing_credentials() {
    let base = token_endpoint().await;
    let client = crate::reqwest_client_builder().build().unwrap();
    let previous = CodexTokens {
        access_token: "access".into(),
        refresh_token: Some("refresh".into()),
        id_token: None,
        account_id: None,
    };
    let missing = crate::model::registry::missing_credentials_error("codex").to_string();
    for (status, code, refused) in [
        (400, "invalid_grant", true),
        (400, "invalid_request", false),
        (401, "refresh_token_expired", true),
        (403, "forbidden", false),
        (503, "unavailable", false),
    ] {
        let error = refresh_codex_token_at(
            &client,
            &MemoryCredentialStore::default(),
            "refresh",
            CodexAuthSource::Store,
            &previous,
            &format!("{base}/{status}/{code}"),
        )
        .await
        .expect_err("refresh fails");
        let reported =
            matches!(&error, ModelError::MissingCredentials(_)).then(|| error.to_string());
        assert_eq!(
            reported,
            refused.then(|| missing.clone()),
            "HTTP {status} {code}"
        );
    }
}
