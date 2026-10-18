use super::*;
use crate::credentials::MemoryCredentialStore;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

/// Answers every request to `/<status>` with that status and an OAuth error.
async fn token_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let status = line.split_whitespace().nth(1).unwrap()[1..].to_owned();
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
                let body = r#"{"error":"invalid_grant"}"#;
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
// an endpoint outage or proxy block stays an HTTP failure.
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
    for (status, refused) in [(400, true), (401, true), (403, false), (503, false)] {
        let error = refresh_codex_token_at(
            &client,
            &MemoryCredentialStore::default(),
            "refresh",
            CodexAuthSource::Store,
            &previous,
            &format!("{base}/{status}"),
        )
        .await
        .expect_err("refresh fails");
        let reported =
            matches!(&error, ModelError::MissingCredentials(_)).then(|| error.to_string());
        assert_eq!(reported, refused.then(|| missing.clone()), "HTTP {status}");
    }
}
