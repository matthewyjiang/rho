use serde_json::{json, Value};

use {
    crate::config::{Config, SearchProvider},
    rho_tools::tool::{Tool, ToolContext},
};

use super::{
    adapters::GetSearchContent,
    fetch::github::{self, GitHubKind},
    search::{self, SearchItem},
    storage::{self, StoredItem},
};

fn test_context() -> ToolContext {
    ToolContext {
        cwd: tempfile::tempdir().unwrap().keep(),
        max_output_bytes: 12000,
    }
}

#[test]
fn parses_github_root_tree_blob_and_commit_urls() {
    let root = github::parse_url("https://github.com/owner/repo").unwrap();
    assert_eq!(root.owner, "owner");
    assert_eq!(root.repo, "repo");
    assert_eq!(root.kind, GitHubKind::Root);

    let tree = github::parse_url("https://github.com/owner/repo/tree/main/src/tools").unwrap();
    assert_eq!(tree.kind, GitHubKind::Tree);
    assert_eq!(tree.ref_name.as_deref(), Some("main"));
    assert_eq!(tree.path, "src/tools");

    let slashed_ref =
        github::parse_url("https://github.com/owner/repo/tree/feature/foo/src/tools").unwrap();
    assert_eq!(slashed_ref.ref_name.as_deref(), Some("feature/foo"));
    assert_eq!(slashed_ref.path, "src/tools");

    let blob = github::parse_url("https://github.com/owner/repo/blob/main/README.md").unwrap();
    assert_eq!(blob.kind, GitHubKind::Blob);
    assert_eq!(blob.path, "README.md");

    let commit = github::parse_url("https://github.com/owner/repo/commit/abc123").unwrap();
    assert_eq!(commit.kind, GitHubKind::Commit);
    assert_eq!(commit.ref_name.as_deref(), Some("abc123"));
}

#[tokio::test]
async fn web_search_stores_stub_content_when_provider_is_unavailable() {
    let args = json!({"query": "rho web access", "provider": "tavily", "includeContent": true});
    let ctx = test_context();
    let web_search = super::access_tools(&Config::default());
    let result = web_search.call(args, ctx, "call_1".into()).await.unwrap();
    let value: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(value["fullContentAvailable"], false);
    assert_eq!(value["sourceContentAvailable"], false);
    assert_eq!(value["storedContentAvailable"], true);
    let response_id = value["responseId"].as_str().unwrap();

    let retrieved = GetSearchContent
        .call(
            json!({"responseId": response_id, "queryIndex": 0}),
            test_context(),
            "call_2".into(),
        )
        .await
        .unwrap();
    assert!(retrieved.content.contains("No configured search provider"));
}

#[tokio::test]
async fn search_item_content_preserves_snippet_when_fetch_fails() {
    let item = SearchItem {
        title: Some("example".into()),
        url: Some("ftp://example.com/article".into()),
        snippet: "original snippet".into(),
    };

    let (content, content_kind) =
        search::item_content(&super::util::http_client(), &item, true).await;

    assert_eq!(content_kind, "snippet_with_fetch_warning");
    assert!(content.contains("original snippet"));
    assert!(content.contains("content fetch failed"));
}

#[test]
fn content_availability_matches_stored_content_kind() {
    let items = vec![
        StoredItem {
            url: Some("https://example.com".into()),
            query: Some("example".into()),
            title: Some("failed".into()),
            content: "content fetch failed".into(),
            metadata: json!({"contentKind": "fetch_failed"}),
        },
        StoredItem {
            url: Some("https://example.net".into()),
            query: Some("example".into()),
            title: Some("snippet preserved".into()),
            content: "original snippet\n\ncontent fetch failed".into(),
            metadata: json!({"contentKind": "snippet_with_fetch_warning"}),
        },
        StoredItem {
            url: Some("https://example.org".into()),
            query: Some("example".into()),
            title: Some("source".into()),
            content: "source page".into(),
            metadata: json!({"contentKind": "source_page"}),
        },
    ];

    let all = storage::content_availability(&items);
    assert!(all.sources);
    assert!(all.snippets);
    assert!(!storage::content_availability(&items[..2]).sources);
    assert!(!storage::content_availability(&items[..1]).snippets);
}

#[tokio::test]
async fn get_search_content_rejects_invalid_response_id() {
    let err = GetSearchContent
        .call(
            json!({"responseId": "../00000000000000000000000000000000"}),
            test_context(),
            "call_1".into(),
        )
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "invalid responseId: expected 32 lowercase hexadecimal characters"
    );
}

#[test]
fn search_provider_parses_tool_and_config_values() {
    assert_eq!("openai".parse(), Ok(SearchProvider::OpenAi));
    assert_eq!(
        SearchProvider::from_config_value("unknown"),
        SearchProvider::Auto
    );
    assert_eq!(
        SearchProvider::Brave.next_configurable(),
        SearchProvider::Disabled
    );
}

#[test]
fn tool_specs_and_fetch_security_preserve_public_contract() {
    assert_eq!(
        super::access_tools(&Config::default()).spec().name,
        "web_search"
    );
    let fetch_content = super::SdkFetchContent::new(12_000);
    assert_eq!(
        rho_sdk::tool::Tool::spec(&fetch_content).name,
        "fetch_content"
    );
    assert_eq!(
        rho_sdk::tool::Tool::security(&fetch_content).capabilities(),
        [
            rho_sdk::CapabilityKind::Read,
            rho_sdk::CapabilityKind::Process,
            rho_sdk::CapabilityKind::Network,
        ]
    );
    assert_eq!(GetSearchContent.spec().name, "get_search_content");
}

#[tokio::test]
async fn fetch_url_text_truncates_large_bodies_without_utf8_errors() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        drop(reader);
        let body = "あ".repeat(700_000);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body.as_bytes());
    });

    let client = reqwest::Client::new();
    let url = format!("http://{address}/big");
    let result = super::fetch::fetch_url_text(&client, &url).await;
    server.join().unwrap();

    let content = result.expect("truncated fetch of valid UTF-8 must not fail");
    assert_eq!(content.len(), 2_097_150);
    assert!(content.chars().all(|c| c == 'あ'));
}

#[tokio::test]
async fn fetch_url_text_rejects_invalid_utf8_below_the_byte_cap() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        drop(reader);
        let body = b"ok\xe3\x81";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    });

    let client = reqwest::Client::new();
    let url = format!("http://{address}/small");
    let result = super::fetch::fetch_url_text(&client, &url).await;
    server.join().unwrap();

    assert!(matches!(result, Err(rho_tools::tool::ToolError::Utf8(_))));
}

#[tokio::test]
async fn fetch_url_text_rejects_invalid_utf8_at_the_byte_cap() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        drop(reader);
        let mut body = vec![b'a'; 2 * 1024 * 1024 - 2];
        body.extend_from_slice(b"\xe3\x81");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(&body);
    });

    let client = reqwest::Client::new();
    let url = format!("http://{address}/exact-cap");
    let result = super::fetch::fetch_url_text(&client, &url).await;
    server.join().unwrap();

    assert!(matches!(result, Err(rho_tools::tool::ToolError::Utf8(_))));
}
