use serde_json::{json, Value};

use {
    crate::config::Config,
    rho_tools::tool::{Tool, ToolContext},
};

use super::{
    adapters::GetSearchContent,
    fetch::github::{self, GitHubKind},
    search::{self, SearchItem},
    storage::{self, StoredItem, WebAccessStore},
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

    let special_ref =
        github::parse_url("https://github.com/owner/repo/tree/hello-$USER/src/tools").unwrap();
    assert_eq!(special_ref.ref_name.as_deref(), Some("hello-$USER"));
    assert_eq!(special_ref.path, "src/tools");

    let plus_ref =
        github::parse_url("https://github.com/owner/repo/blob/feature+api/README.md").unwrap();
    assert_eq!(plus_ref.kind, GitHubKind::Blob);
    assert_eq!(plus_ref.ref_name.as_deref(), Some("feature+api"));
    assert_eq!(plus_ref.path, "README.md");
}

#[test]
fn rejects_github_urls_whose_segments_could_inject_git_arguments() {
    for url in [
        "https://github.com/-owner/repo",
        "https://github.com/owner/-repo",
        "https://github.com/owner/re;po",
        "https://github.com/owner/repo/tree/--upload-pack=touch%20pwned",
        "https://github.com/owner/repo/commit/--output=pwned",
        "https://github.com/owner/repo/tree/%2e%2e/etc",
    ] {
        assert!(
            github::parse_url(url).is_none(),
            "{url} should not parse as a GitHub target"
        );
    }
}

#[tokio::test]
async fn web_search_stores_stub_content_when_provider_is_unavailable() {
    let args = json!({"query": "rho web access", "provider": "tavily", "includeContent": true});
    let ctx = test_context();
    let store = WebAccessStore::new();
    let web_search = super::access_tools_with_store(&Config::default(), store.clone());
    let result = web_search.call(args, ctx, "call_1".into()).await.unwrap();
    let value: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(value["fullContentAvailable"], false);
    assert_eq!(value["sourceContentAvailable"], false);
    assert_eq!(value["storedContentAvailable"], true);
    let response_id = value["responseId"].as_str().unwrap();

    let retrieved = GetSearchContent::new(store)
        .call(
            json!({"responseId": response_id, "queryIndex": 0}),
            test_context(),
            "call_2".into(),
        )
        .await
        .unwrap();
    assert!(retrieved.content.contains("No configured search provider"));
}

#[test]
fn web_search_available_with_hosted_only_backup_disabled() {
    let config = Config {
        provider: "openai-codex".into(),
        model: "gpt-5.5".into(),
        web_search_hosted: true,
        web_search_provider: crate::config::SearchProvider::Disabled,
        ..Config::default()
    };
    assert!(super::web_search_available(&config));
    assert!(super::hosted_web_search_active(&config));
    assert!(!super::backup_web_search_available(&config));
}

#[test]
fn web_search_unavailable_when_hosted_and_backup_disabled() {
    let config = Config {
        provider: "openai-codex".into(),
        model: "gpt-5.5".into(),
        web_search_hosted: false,
        web_search_provider: crate::config::SearchProvider::Disabled,
        ..Config::default()
    };
    assert!(!super::web_search_available(&config));
}

#[test]
fn codex_lite_uses_backup_not_hosted() {
    let config = Config {
        provider: "openai-codex".into(),
        model: "gpt-5.6-luna".into(),
        web_search_hosted: true,
        web_search_provider: crate::config::SearchProvider::Disabled,
        ..Config::default()
    };
    assert!(!super::hosted_web_search_active(&config));
    assert!(!super::web_search_available(&config));
}

#[test]
fn xai_supports_hosted_web_search() {
    assert!(super::supports_hosted_web_search("xai", "grok-4.5"));
}

#[tokio::test]
async fn search_item_content_preserves_snippet_when_fetch_fails() {
    let item = SearchItem {
        title: Some("example".into()),
        url: Some("ftp://example.com/article".into()),
        snippet: "original snippet".into(),
    };

    let (content, content_kind) = search::item_content(&item, true).await;

    assert_eq!(content_kind, "snippet_with_fetch_warning");
    assert!(content.contains("original snippet"));
    assert!(content.contains("content fetch failed"));
}

#[tokio::test]
async fn get_search_content_lists_available_selectors_on_query_miss() {
    let store = WebAccessStore::new();
    let response_id = storage::new_response_id();
    store
        .store(
            response_id.clone(),
            storage::StoredContent {
                kind: "fetch_content".into(),
                items: vec![StoredItem {
                    url: Some("https://example.com/doc".into()),
                    query: Some("exact prompt".into()),
                    title: None,
                    content: "body".into(),
                    metadata: json!({}),
                }],
            },
        )
        .unwrap();

    let err = GetSearchContent::new(store)
        .call(
            json!({
                "responseId": response_id,
                "query": "--allowedTools"
            }),
            test_context(),
            "call_1".into(),
        )
        .await
        .unwrap_err();

    let message = err.to_string();
    assert!(message.contains("query must equal an original"));
    assert!(message.contains("https://example.com/doc"));
    assert!(message.contains("exact prompt"));
}

#[tokio::test]
async fn get_search_content_rejects_invalid_response_id() {
    let err = GetSearchContent::new(WebAccessStore::new())
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

    let url = format!("http://{address}/big");
    let loopback = vec![super::ssrf::Cidr::parse("127.0.0.0/8").unwrap()];
    let result = super::ssrf::with_allow_ranges(loopback, async {
        super::fetch::fetch_url_text(&url).await
    })
    .await;
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

    let url = format!("http://{address}/small");
    let loopback = vec![super::ssrf::Cidr::parse("127.0.0.0/8").unwrap()];
    let result = super::ssrf::with_allow_ranges(loopback, async {
        super::fetch::fetch_url_text(&url).await
    })
    .await;
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

    let url = format!("http://{address}/exact-cap");
    let loopback = vec![super::ssrf::Cidr::parse("127.0.0.0/8").unwrap()];
    let result = super::ssrf::with_allow_ranges(loopback, async {
        super::fetch::fetch_url_text(&url).await
    })
    .await;
    server.join().unwrap();

    assert!(matches!(result, Err(rho_tools::tool::ToolError::Utf8(_))));
}

// Covers: an extensionless PDF truncated at a caller-selected transport limit remains identified
// as a PDF, so callers can reject partial document extraction.
// Owner: fetch_content transport
#[tokio::test]
async fn byte_limited_fetch_marks_magic_byte_pdf_as_truncated() {
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
        let body = b"%PDF-1.4 truncated fixture";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });

    let url = format!("http://{address}/download");
    let loopback = vec![super::ssrf::Cidr::parse("127.0.0.0/8").unwrap()];
    let downloaded = super::ssrf::with_allow_ranges(loopback, async {
        super::fetch::fetch_url_bytes(&url, 8).await
    })
    .await
    .unwrap();
    server.join().unwrap();

    assert_eq!(downloaded.bytes, b"%PDF-1.4");
    assert!(downloaded.truncated);
    assert_eq!(
        downloaded.pdf_detection,
        super::fetch::PdfDetection::MagicBytes
    );
}

#[tokio::test]
async fn fetch_url_text_blocks_loopback_by_default() {
    let error = super::fetch::fetch_url_text("http://127.0.0.1:9/")
        .await
        .expect_err("loopback must be refused");
    assert!(
        error.to_string().contains("blocked"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn fetch_url_text_refuses_redirect_responses() {
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
        let response = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/private\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(response.as_bytes());
    });

    let url = format!("http://{address}/public");
    let loopback = vec![super::ssrf::Cidr::parse("127.0.0.0/8").unwrap()];
    let error = super::ssrf::with_allow_ranges(loopback, async {
        super::fetch::fetch_url_text(&url).await
    })
    .await
    .expect_err("redirect responses must be refused");
    server.join().unwrap();

    assert!(
        error.to_string().contains("refusing to follow redirect"),
        "unexpected error: {error}"
    );
}

// Covers: local and remote PDF fetches must return structured Markdown instead of a placeholder or
// a UTF-8 decode error.
// Owner: fetch_content transport integration
#[tokio::test]
async fn fetch_content_extracts_local_and_remote_pdf_text() {
    use std::{
        io::{BufRead, BufReader, Write as _},
        net::TcpListener,
        thread,
    };

    fn pdf_fixture() -> Vec<u8> {
        let stream = "BT /F1 12 Tf 72 720 Td (Fetched PDF) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            write!(pdf, "{} 0 obj\n{}\nendobj\n", index + 1, object).unwrap();
        }
        let xref_offset = pdf.len();
        write!(pdf, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
        for offset in offsets {
            writeln!(pdf, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            pdf,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .unwrap();
        pdf
    }

    let pdf = pdf_fixture();
    let directory = tempfile::tempdir().unwrap();
    let local_path = directory.path().join("local.pdf");
    std::fs::write(&local_path, &pdf).unwrap();
    let local = super::fetch::fetch_local_path(&local_path, None, None, 1)
        .await
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_pdf = pdf.clone();
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
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
            server_pdf.len()
        )
        .unwrap();
        stream.write_all(&server_pdf).unwrap();
    });
    let url = url::Url::parse(&format!("http://{address}/download")).unwrap();
    let loopback = vec![super::ssrf::Cidr::parse("127.0.0.0/8").unwrap()];
    let remote = super::ssrf::with_allow_ranges(loopback, async {
        super::fetch::fetch_http_url(&url, None).await
    })
    .await
    .unwrap();
    server.join().unwrap();

    assert!(
        local.content.contains("Fetched PDF"),
        "local PDF extract should keep the text layer: {}",
        local.content
    );
    assert!(
        remote.content.contains("Fetched PDF"),
        "remote PDF extract should keep the text layer: {}",
        remote.content
    );
    assert_eq!(local.metadata["mode"], "document_extract");
    assert_eq!(remote.metadata["mode"], "document_extract");
}

/// Serves one plain-text response on loopback and reports the `Host` header it
/// received, so pinning tests can prove the request kept its original hostname.
fn serve_once_reporting_host(
    body: &'static str,
) -> (
    std::net::SocketAddr,
    std::thread::JoinHandle<Option<String>>,
) {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut host = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("host") {
                    host = Some(value.trim().to_string());
                }
            }
        }
        drop(reader);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        host
    });
    (address, server)
}

/// DNS rebinding regression: the hostname resolves to a permitted address for
/// the SSRF check and to the cloud metadata address afterwards. The fetch must
/// connect to the checked address and never resolve the name a second time.
///
/// `pinned.invalid` cannot resolve through the system resolver, so a successful
/// fetch is only possible through the pinned address.
#[tokio::test]
async fn fetch_url_text_pins_the_connection_to_the_vetted_address() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (address, server) = serve_once_reporting_host("pinned body");
    let lookups = std::sync::Arc::new(AtomicUsize::new(0));
    let answers = lookups.clone();
    let url = format!("http://pinned.invalid:{}/page", address.port());
    let loopback = vec![super::ssrf::Cidr::parse("127.0.0.0/8").unwrap()];

    let content = super::ssrf::with_resolver(
        move |_hostname| {
            if answers.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![address.ip()]
            } else {
                vec!["169.254.169.254".parse().unwrap()]
            }
        },
        super::ssrf::with_allow_ranges(loopback, async {
            super::fetch::fetch_url_text(&url).await
        }),
    )
    .await
    .expect("fetch must reach the vetted address");
    let host = server.join().unwrap();

    assert_eq!(content, "pinned body");
    assert_eq!(lookups.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.as_deref(),
        Some(format!("pinned.invalid:{}", address.port()).as_str())
    );
}

/// Every connection candidate comes from the validated answer set: the first
/// vetted address has no listener, so the fetch falls through to the second.
///
/// The unreachable candidate is IPv6 loopback, which refuses at once on every
/// platform. `127.0.0.2` is not routed on macOS, so it stalls instead.
#[tokio::test]
async fn fetch_url_text_tries_every_vetted_address() {
    let (address, server) = serve_once_reporting_host("second answer");
    let candidates = vec!["::1".parse().unwrap(), address.ip()];
    let url = format!("http://pinned.invalid:{}/page", address.port());
    let loopback = vec![
        super::ssrf::Cidr::parse("127.0.0.0/8").unwrap(),
        super::ssrf::Cidr::parse("::1/128").unwrap(),
    ];

    let content = super::ssrf::with_resolver(
        move |_hostname| candidates.clone(),
        super::ssrf::with_allow_ranges(loopback, async {
            super::fetch::fetch_url_text(&url).await
        }),
    )
    .await
    .expect("fetch must fall through to the reachable vetted address");
    let host = server.join().unwrap();

    assert_eq!(content, "second answer");
    assert_eq!(
        host.as_deref(),
        Some(format!("pinned.invalid:{}", address.port()).as_str())
    );
}

/// A blocked answer still fails the whole target, even mixed with public ones.
#[tokio::test]
async fn fetch_url_text_rejects_targets_with_any_blocked_answer() {
    let error = super::ssrf::with_resolver(
        |_hostname| {
            vec![
                "93.184.216.34".parse().unwrap(),
                "169.254.169.254".parse().unwrap(),
            ]
        },
        super::fetch::fetch_url_text("http://mixed.invalid/page"),
    )
    .await
    .expect_err("a blocked answer must reject the fetch");

    assert!(
        error.to_string().contains("blocked"),
        "unexpected error: {error}"
    );
}
