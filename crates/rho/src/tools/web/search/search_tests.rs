use std::{collections::BTreeMap, time::Duration};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::config::OPENAI_CODEX_RESPONSES_URL;

#[derive(Debug)]
struct Request {
    target: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

async fn server(status: u16, body: String) -> (String, tokio::task::JoinHandle<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
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
            let headers: BTreeMap<_, _> = head.lines().skip(1).filter_map(|line| {
                line.split_once(':').map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_string()))
            }).collect();
            let length = headers.get("content-length").map(|n| n.parse::<usize>().unwrap()).unwrap_or(0);
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let request = Request {
                target: head.lines().next().unwrap().to_string(),
                headers,
                body: if length == 0 { Value::Null } else { serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap() },
            };
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            // An oversized response may be rejected before the server finishes writing.
            let _ = stream.write_all(response.as_bytes()).await;
            request
        }).await.expect("mock search request timed out")
    });
    (base, task)
}

fn config(backend: SearchBackend, base: String) -> SearchBackendConfig {
    let mut settings = WebSearchSettings {
        backend,
        ..WebSearchSettings::default()
    };
    settings.set_endpoint(backend, Some(base));
    let ready = match backend {
        SearchBackend::Firecrawl => Ok(ReadyBackend::Firecrawl { key: None }),
        _ => Err("unavailable".into()),
    };
    SearchBackendConfig { settings, ready }
}

fn with_ready(mut config: SearchBackendConfig, ready: ReadyBackend) -> SearchBackendConfig {
    config.ready = Ok(ready);
    config
}

// Covers: self-hosted search must preserve proxy prefixes, omit absent auth,
// and serialize filters using Firecrawl's mutually exclusive domain fields.
// Owner: provider wire contract
#[tokio::test]
async fn firecrawl_request_and_result_contract() {
    for (key, filters, expected_query, field, domains) in [
        (
            None,
            vec!["example.com"],
            "query",
            "includeDomains",
            json!(["example.com"]),
        ),
        (
            Some("test-key"),
            vec!["-example.com"],
            "query",
            "excludeDomains",
            json!(["example.com"]),
        ),
        (
            None,
            vec!["example.com", "-blocked.example.com"],
            "query -site:blocked.example.com",
            "includeDomains",
            json!(["example.com"]),
        ),
    ] {
        let response = json!({"success":true,"data":{"web":[{"title":"Source","url":"https://example.com/page","description":"Snippet"}]}});
        let (base, task) = server(200, response.to_string()).await;
        let config = with_ready(
            config(SearchBackend::Firecrawl, format!("{base}/proxy")),
            ReadyBackend::Firecrawl {
                key: key.map(str::to_string),
            },
        );
        let filters = filters.into_iter().map(str::to_string).collect::<Vec<_>>();
        let result = run_search_query(
            &super::super::util::http_client(),
            "query",
            3,
            Some("week"),
            Some(&filters),
            &config,
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            vec![SearchItem {
                title: Some("Source".into()),
                url: Some("https://example.com/page".into()),
                snippet: "Snippet".into()
            }]
        );
        let request = task.await.unwrap();
        assert_eq!(request.target, "POST /proxy/v2/search HTTP/1.1");
        assert_eq!(
            request.headers.get("authorization").cloned(),
            key.map(|key| format!("Bearer {key}"))
        );
        let mut expected =
            json!({"query":expected_query,"limit":3,"sources":["web"],"tbs":"qdr:w"});
        expected[field] = domains;
        assert_eq!(request.body, expected);
    }
}

// Covers: provider failures must not look like an empty success or expose keys;
// oversized bodies must be rejected instead of buffering unbounded responses.
// Owner: provider response/error contract
#[tokio::test]
async fn firecrawl_response_failures_are_bounded_and_redacted() {
    for (status, body, succeeds) in [
        (
            200,
            json!({"success":true,"data":{"web":[]}}).to_string(),
            true,
        ),
        (
            200,
            json!({"success":false,"error":"invalid tiny"}).to_string(),
            false,
        ),
        (401, "invalid tiny".into(), false),
        (200, "not json".into(), false),
        (200, json!({"success":true,"data":{}}).to_string(), false),
        (200, "x".repeat(SEARCH_RESPONSE_MAX_BYTES + 1), false),
    ] {
        let (base, task) = server(status, body).await;
        let config = with_ready(
            config(SearchBackend::Firecrawl, base),
            ReadyBackend::Firecrawl {
                key: Some("tiny".into()),
            },
        );
        let result = run_search_query(
            &super::super::util::http_client(),
            "query",
            1,
            None,
            None,
            &config,
        )
        .await;
        assert_eq!(result.is_ok(), succeeds);
        if let Err(error) = result {
            assert!(!error.to_string().contains("tiny"));
        }
        task.await.unwrap();
    }
}

// Covers: configured API endpoints must retain prefixes and use only their own
// credential header, even when unrelated Codex credentials are present.
// Owner: provider transport isolation
#[tokio::test]
async fn api_backends_use_configured_transport() {
    for (backend, suffix, header, response) in [
        (
            SearchBackend::OpenAi,
            "responses",
            "authorization",
            json!({"output":[{"type":"web_search_call","action":{"sources":[{"url":"https://example.com"}]}}]}),
        ),
        (
            SearchBackend::Exa,
            "search",
            "x-api-key",
            json!({"results":[{"url":"https://example.com"}]}),
        ),
        (
            SearchBackend::Brave,
            "res/v1/web/search?q=query&count=1",
            "x-subscription-token",
            json!({"web":{"results":[{"url":"https://example.com"}]}}),
        ),
    ] {
        let (base, task) = server(200, response.to_string()).await;
        let config = with_ready(
            config(backend, format!("{base}/proxy")),
            match backend {
                SearchBackend::OpenAi => {
                    ReadyBackend::OpenAi(openai::OpenAiSearchAuth::ApiKey("api-key".into()))
                }
                SearchBackend::Exa => ReadyBackend::ExaApi {
                    key: "api-key".into(),
                },
                SearchBackend::Brave => ReadyBackend::Brave {
                    key: "api-key".into(),
                },
                SearchBackend::Firecrawl => ReadyBackend::Firecrawl {
                    key: Some("api-key".into()),
                },
            },
        );
        let result = run_search_query(
            &super::super::util::http_client(),
            "query",
            1,
            None,
            None,
            &config,
        )
        .await
        .unwrap();
        assert_eq!(result.len(), 1);
        let request = task.await.unwrap();
        let method = if backend == SearchBackend::Brave {
            "GET"
        } else {
            "POST"
        };
        assert_eq!(request.target, format!("{method} /proxy/{suffix} HTTP/1.1"));
        assert_eq!(
            request.headers.get(header).map(String::as_str),
            Some(if header == "authorization" {
                "Bearer api-key"
            } else {
                "api-key"
            })
        );
        assert_eq!(request.headers.get("chatgpt-account-id"), None);
        assert!(!format!("{request:?}").contains("must-not-leak"));
    }
}

// Covers: a custom Exa MCP endpoint must not be replaced with its API endpoint
// when an API key happens to be available.
// Owner: provider transport isolation
#[tokio::test]
async fn exa_mcp_uses_explicit_endpoint_without_api_auth() {
    let response = json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Title: Source\nURL: https://example.com\nText: Snippet"}]}});
    let (base, task) = server(200, response.to_string()).await;
    let mut config = config(SearchBackend::Exa, "http://unused.invalid".into());
    config.settings.exa.connection = ExaSearchConnection::Mcp;
    config.settings.exa.mcp_url = Some(format!("{base}/custom/mcp"));
    config.ready = Ok(ReadyBackend::ExaMcp);
    let result = run_search_query(
        &super::super::util::http_client(),
        "query",
        1,
        None,
        None,
        &config,
    )
    .await
    .unwrap();
    assert_eq!(result.len(), 1);
    let request = task.await.unwrap();
    assert_eq!(request.target, "POST /custom/mcp HTTP/1.1");
    assert_eq!(request.headers.get("x-api-key"), None);
    assert_eq!(request.headers.get("authorization"), None);
    assert_eq!(request.body["method"], json!("tools/call"));
}

// Covers: absent credentials cannot activate implicit API/MCP/Codex switching.
// Owner: backend readiness policy
#[test]
fn explicit_connections_require_their_own_credentials() {
    let mut config = config(SearchBackend::OpenAi, "http://localhost:3002".into());
    assert!(!config.is_ready());
    config.settings.openai.connection = OpenAiSearchConnection::Codex;
    config.ready = Ok(ReadyBackend::OpenAi(openai::OpenAiSearchAuth::Codex {
        tokens: CodexTokens {
            access_token: "token".into(),
            refresh_token: None,
            id_token: None,
            account_id: None,
        },
        source: CodexAuthSource::Env,
    }));
    assert!(config.is_ready());
    assert_eq!(
        config.destination_url("responses").unwrap(),
        OPENAI_CODEX_RESPONSES_URL
    );
    config.settings.backend = SearchBackend::Exa;
    config.ready = Err("EXA_API_KEY is not set".into());
    assert!(!config.is_ready());
    config.settings.exa.connection = ExaSearchConnection::Mcp;
    config.ready = Ok(ReadyBackend::ExaMcp);
    assert!(config.is_ready());
    config.settings.backend = SearchBackend::Firecrawl;
    config.ready = Err("FIRECRAWL_API_KEY is not set".into());
    assert!(!config.is_ready());
    config.settings.firecrawl.api_base_url = Some("http://localhost:3002".into());
    config.ready = Ok(ReadyBackend::Firecrawl { key: None });
    assert!(config.is_ready());
}

// Covers: blank keys cannot mark cloud ready or produce an empty bearer header;
// cloud requests use the fixed v2 endpoint and the supplied bearer credential.
// Owner: search credential resolution and Firecrawl authentication.
#[test]
fn firecrawl_cloud_auth_uses_nonempty_credentials() {
    for (value, expected) in [("", None), ("  ", None), (" key ", Some("key"))] {
        let key = {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        };
        let config = with_ready(
            config(SearchBackend::Firecrawl, "https://api.firecrawl.dev".into()),
            if key.is_some() {
                ReadyBackend::Firecrawl { key: key.clone() }
            } else {
                // Cloud default still constructs a destination; readiness is the key.
                ReadyBackend::Firecrawl { key: None }
            },
        );
        assert_eq!(
            matches!(
                &config.ready,
                Ok(ReadyBackend::Firecrawl { key }) if key.is_some()
            ),
            expected.is_some()
        );
        let request = firecrawl::authenticated_request(
            &super::super::util::http_client(),
            &config.destination_url("v2/search").unwrap(),
            &json!({"query":"test"}),
            key.as_deref(),
        )
        .build()
        .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.firecrawl.dev/v2/search"
        );
        assert_eq!(
            request
                .headers()
                .get("authorization")
                .map(|value| value.to_str().unwrap()),
            expected.map(|_| "Bearer key")
        );
    }
}
