use std::{collections::HashMap, sync::Mutex};

use pretty_assertions::assert_eq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::credentials::CredentialResult;

#[derive(Default)]
struct TestStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl CredentialStore for TestStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        Ok(self.secrets.lock().unwrap().get(account).cloned())
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.secrets
            .lock()
            .unwrap()
            .insert(account.into(), secret.into());
        Ok(())
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        Ok(self.secrets.lock().unwrap().remove(account).is_some())
    }
}

#[test]
fn parses_only_windows_reported_by_codex() {
    let payload: CodexUsagePayload = serde_json::from_value(serde_json::json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 31,
                "limit_window_seconds": 604800,
                "reset_after_seconds": 300,
                "reset_at": 1_800_000_000
            },
            "secondary_window": null
        }
    }))
    .unwrap();

    let windows = payload
        .rate_limit
        .into_iter()
        .flat_map(|limits| [limits.primary_window, limits.secondary_window])
        .flatten()
        .map(UsageLimitWindow::from)
        .collect::<Vec<_>>();

    assert_eq!(
        windows,
        vec![UsageLimitWindow {
            label: "Weekly".into(),
            remaining_percent: 69.0,
            resets_at_unix: 1_800_000_000,
        }]
    );
}

#[test]
fn labels_returned_windows_by_duration_instead_of_position() {
    assert_eq!(window_label(18_000), "5-hour");
    assert_eq!(window_label(604_800), "Weekly");
    assert_eq!(window_label(86_400), "Daily");
    assert_eq!(window_label(172_800), "2-day");
}

#[tokio::test]
async fn codex_source_sends_oauth_and_account_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/usage", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let count = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
        assert!(request.contains("authorization: bearer access-token"));
        assert!(request.contains("chatgpt-account-id: account-123"));
        let body = r#"{"rate_limit":{"primary_window":{"used_percent":25,"limit_window_seconds":18000,"reset_at":1800000000},"secondary_window":null}}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });

    let source = CodexUsageLimitsSource::with_endpoint(endpoint);
    let limits = source
        .fetch_with_tokens(
            &TestStore::default(),
            CodexTokens {
                access_token: "access-token".into(),
                refresh_token: None,
                id_token: None,
                account_id: Some("account-123".into()),
            },
            CodexAuthSource::Store,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(
        limits,
        ProviderLimits {
            provider: "Codex".into(),
            windows: vec![UsageLimitWindow {
                label: "5-hour".into(),
                remaining_percent: 75.0,
                resets_at_unix: 1_800_000_000,
            }],
        }
    );
}
