use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use pretty_assertions::assert_eq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Barrier,
};

use super::*;
use rho_providers::credentials::{save_kimi_tokens, save_xai_tokens, CredentialResult};

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

struct ConcurrentSource {
    barrier: Arc<Barrier>,
    limits: ProviderUsageLimits,
}

impl UsageLimitsSource for ConcurrentSource {
    fn fetch<'a>(
        &'a self,
        _store: &'a dyn CredentialStore,
    ) -> Pin<
        Box<dyn Future<Output = Result<Option<ProviderUsageLimits>, UsageLimitsError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.barrier.wait().await;
            Ok(Some(self.limits.clone()))
        })
    }
}

#[tokio::test]
async fn fetches_connected_providers_concurrently_in_alphabetical_order() {
    let barrier = Arc::new(Barrier::new(2));
    let first = ConcurrentSource {
        barrier: barrier.clone(),
        limits: ProviderUsageLimits {
            provider: "Zulu".into(),
            windows: Vec::new(),
        },
    };
    let second = ConcurrentSource {
        barrier,
        limits: ProviderUsageLimits {
            provider: "alpha".into(),
            windows: Vec::new(),
        },
    };

    let (limits, errors) = tokio::time::timeout(
        Duration::from_secs(1),
        fetch_usage_limits_from_sources(&TestStore::default(), &first, &second),
    )
    .await
    .expect("both provider fetches should start before either completes")
    .unwrap();

    assert_eq!(
        limits,
        ProviderLimits {
            providers: vec![second.limits, first.limits],
        }
    );
    assert!(errors.is_empty());
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

#[test]
fn parses_only_weekly_window_reported_by_xai_billing() {
    let payload: XaiBillingPayload = serde_json::from_value(serde_json::json!({
        "config": {
            "currentPeriod": {
                "type": "USAGE_PERIOD_TYPE_WEEKLY",
                "start": "2026-07-04T02:57:36.331252+00:00",
                "end": "2026-07-11T02:57:36.331252+00:00"
            },
            "creditUsagePercent": 3.0,
            "productUsage": [
                {"product": "GrokBuild", "usagePercent": 2.0},
                {"product": "GrokChat", "usagePercent": 1.0}
            ]
        }
    }))
    .unwrap();

    assert_eq!(
        payload.windows(),
        vec![UsageLimitWindow {
            label: "Weekly".into(),
            remaining_percent: 97.0,
            resets_at_unix: chrono::DateTime::parse_from_rfc3339(
                "2026-07-11T02:57:36.331252+00:00"
            )
            .unwrap()
            .timestamp(),
        }]
    );
}

#[test]
fn omits_xai_window_when_credit_usage_is_absent() {
    let payload: XaiBillingPayload = serde_json::from_value(serde_json::json!({
        "config": {
            "currentPeriod": {
                "end": "2026-07-11T02:57:36.331252+00:00"
            }
        }
    }))
    .unwrap();

    assert_eq!(payload.windows(), Vec::<UsageLimitWindow>::new());
}

#[test]
fn blank_xai_env_token_falls_back_to_stored_oauth_tokens() {
    let store = TestStore::default();
    save_xai_tokens(
        &store,
        &XaiTokens {
            access_token: "stored-access".into(),
            refresh_token: Some("refresh".into()),
            expires_at_unix: None,
            id_token: None,
        },
    )
    .unwrap();

    assert_eq!(
        XaiUsageLimitsSource::configured_tokens_from(&store, Some("  ".into())).unwrap(),
        Some((
            XaiTokens {
                access_token: "stored-access".into(),
                refresh_token: Some("refresh".into()),
                expires_at_unix: None,
                id_token: None,
            },
            XaiAuthSource::Store,
        ))
    );
}

#[test]
fn labels_xai_window_from_reported_period_type() {
    let payload: XaiBillingPayload = serde_json::from_value(serde_json::json!({
        "config": {
            "currentPeriod": {
                "type": "USAGE_PERIOD_TYPE_MONTHLY",
                "end": "2026-07-11T02:57:36Z"
            },
            "creditUsagePercent": 25.0
        }
    }))
    .unwrap();

    assert_eq!(payload.windows()[0].label, "Monthly");
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
        ProviderUsageLimits {
            provider: "Codex".into(),
            windows: vec![UsageLimitWindow {
                label: "5-hour".into(),
                remaining_percent: 75.0,
                resets_at_unix: 1_800_000_000,
            }],
        }
    );
}

#[test]
fn parses_kimi_weekly_and_rolling_windows() {
    let payload: KimiUsagePayload = serde_json::from_value(serde_json::json!({
        "usage": {
            "limit": "100",
            "remaining": "75",
            "resetTime": "2026-02-11T17:32:50.757941Z"
        },
        "limits": [{
            "window": {
                "duration": 300,
                "timeUnit": "TIME_UNIT_MINUTE"
            },
            "detail": {
                "limit": "200",
                "used": "139",
                "resetTime": "2026-02-07T12:32:50.757941Z"
            }
        }]
    }))
    .unwrap();

    assert_eq!(
        payload.windows(),
        vec![
            UsageLimitWindow {
                label: "5-hour".into(),
                remaining_percent: 30.5,
                resets_at_unix: chrono::DateTime::parse_from_rfc3339("2026-02-07T12:32:50.757941Z")
                    .unwrap()
                    .timestamp(),
            },
            UsageLimitWindow {
                label: "Weekly".into(),
                remaining_percent: 75.0,
                resets_at_unix: chrono::DateTime::parse_from_rfc3339("2026-02-11T17:32:50.757941Z")
                    .unwrap()
                    .timestamp(),
            },
        ]
    );
}

#[test]
fn blank_kimi_env_token_falls_back_to_stored_oauth_tokens() {
    let store = TestStore::default();
    let tokens = KimiTokens {
        access_token: "stored-access".into(),
        refresh_token: Some("refresh".into()),
        expires_at_unix: None,
        scope: "kimi:code".into(),
        token_type: "Bearer".into(),
        expires_in: None,
    };
    save_kimi_tokens(&store, &tokens).unwrap();

    assert_eq!(
        KimiUsageLimitsSource::configured_tokens_from(&store, Some("  ".into())).unwrap(),
        Some((tokens, KimiAuthSource::Store))
    );
}

#[tokio::test]
async fn kimi_source_sends_oauth_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/usages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let count = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
        assert!(request.contains("authorization: bearer access-token"));
        assert!(request.contains("accept: application/json"));
        let body = r#"{"usage":{"limit":"100","remaining":"75","resetTime":"2026-02-11T17:32:50.757941Z"},"limits":[]}"#;
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

    let source = KimiUsageLimitsSource::with_endpoint(endpoint);
    let limits = source
        .fetch_with_tokens(
            &TestStore::default(),
            KimiTokens {
                access_token: "access-token".into(),
                refresh_token: None,
                expires_at_unix: None,
                scope: "kimi:code".into(),
                token_type: "Bearer".into(),
                expires_in: None,
            },
            KimiAuthSource::Store,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(
        limits,
        ProviderUsageLimits {
            provider: "Kimi Code".into(),
            windows: vec![UsageLimitWindow {
                label: "Weekly".into(),
                remaining_percent: 75.0,
                resets_at_unix: chrono::DateTime::parse_from_rfc3339("2026-02-11T17:32:50.757941Z")
                    .unwrap()
                    .timestamp(),
            }],
        }
    );
}

#[tokio::test]
async fn xai_source_sends_oauth_cli_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/billing?format=credits",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let count = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..count]).to_ascii_lowercase();
        assert!(request.contains("authorization: bearer access-token"));
        assert!(request.contains("x-xai-token-auth: xai-grok-cli"));
        assert!(request.contains("x-grok-client-version: 0.2.93"));
        let body = r#"{"config":{"creditUsagePercent":12.5,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-07-11T02:57:36Z"}}}"#;
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

    let source = XaiUsageLimitsSource::with_endpoint(endpoint);
    let limits = source
        .fetch_with_tokens(
            &TestStore::default(),
            XaiTokens {
                access_token: "access-token".into(),
                refresh_token: None,
                expires_at_unix: None,
                id_token: None,
            },
            XaiAuthSource::Store,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(
        limits,
        ProviderUsageLimits {
            provider: "xAI".into(),
            windows: vec![UsageLimitWindow {
                label: "Weekly".into(),
                remaining_percent: 87.5,
                resets_at_unix: chrono::DateTime::parse_from_rfc3339("2026-07-11T02:57:36Z")
                    .unwrap()
                    .timestamp(),
            }],
        }
    );
}
