use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use base64::Engine;
use http::{HeaderName, HeaderValue};
use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialStore, MemoryCredentialStore};
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::tools::mcp::config::McpOAuthConfig;

const IDENTITY: &str = "docs";

/// A prompt that fails instead of opening a browser.
///
/// Every test that must not reach a login uses this, so a regression that
/// starts one fails the test rather than opening a window on the machine.
fn no_browser() -> AuthorizationPrompt {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
    drop(receiver);
    AuthorizationPrompt::Captured(sender)
}

fn header_map(pairs: &[(&str, &str)]) -> HashMap<HeaderName, HeaderValue> {
    pairs
        .iter()
        .map(|(name, value)| {
            (
                HeaderName::try_from(*name).unwrap(),
                HeaderValue::try_from(*value).unwrap(),
            )
        })
        .collect()
}

// Failure mode: Rho starts a browser login even though the user already told
// it which credential to send, minting a second identity nobody asked for.
// Owner layer: the MCP OAuth authorization plan.
#[test]
fn a_configured_authorization_header_suppresses_the_oauth_flow() {
    let oauth = McpOAuthConfig::default();
    let cases = [
        (
            "no oauth table",
            None,
            header_map(&[]),
            AuthorizationPlan::Skip,
        ),
        (
            "oauth table alone",
            Some(&oauth),
            header_map(&[]),
            AuthorizationPlan::Run(&oauth),
        ),
        (
            "explicit Authorization header wins",
            Some(&oauth),
            header_map(&[("authorization", "Bearer configured-token")]),
            AuthorizationPlan::Skip,
        ),
        (
            "header name casing does not matter",
            Some(&oauth),
            header_map(&[("Authorization", "Bearer configured-token")]),
            AuthorizationPlan::Skip,
        ),
        (
            "an unrelated header does not suppress",
            Some(&oauth),
            header_map(&[("x-tenant", "acme")]),
            AuthorizationPlan::Run(&oauth),
        ),
    ];

    for (label, configured, headers, expected) in cases {
        assert_eq!(
            authorization_plan(IDENTITY, configured, &headers),
            expected,
            "{label}"
        );
    }
}

// Failure mode: a batch command or CI job blocks on a browser nobody will ever
// open, so `rho mcp list --connect` hangs instead of reporting.
// Owner layer: authorization-mode resolution.
#[test]
fn a_browser_login_needs_a_terminal_and_no_ci_marker() {
    let cases = [
        (
            TerminalAttachment::Attached,
            CiEnvironment::Absent,
            McpAuthorizationMode::Interactive,
        ),
        (
            TerminalAttachment::Attached,
            CiEnvironment::Detected,
            McpAuthorizationMode::NonInteractive,
        ),
        (
            TerminalAttachment::Detached,
            CiEnvironment::Absent,
            McpAuthorizationMode::NonInteractive,
        ),
        (
            TerminalAttachment::Detached,
            CiEnvironment::Detected,
            McpAuthorizationMode::NonInteractive,
        ),
    ];

    for (terminal, ci, expected) in cases {
        assert_eq!(
            McpAuthorizationMode::resolve(terminal, ci),
            expected,
            "{terminal:?} with {ci:?}"
        );
    }
}

// Failure mode: a discovery document moves the token exchange onto plaintext
// HTTP, so the authorization code and the access token cross the network in
// the clear.
// Owner layer: OAuth metadata validation.
#[test]
fn a_plaintext_oauth_endpoint_is_refused() {
    let secure = |authorization: &str, token: &str, registration: Option<&str>, issuer: &str| {
        let metadata: AuthorizationMetadata = serde_json::from_value(serde_json::json!({
            "authorization_endpoint": authorization,
            "token_endpoint": token,
            "registration_endpoint": registration,
            "issuer": issuer,
        }))
        .expect("metadata fixture must parse");
        validate_metadata(&metadata).is_ok()
    };

    assert!(secure(
        "https://auth.example.com/authorize",
        "https://auth.example.com/token",
        Some("https://auth.example.com/register"),
        "https://auth.example.com",
    ));
    // Loopback keeps the same exemption the MCP URL itself gets.
    assert!(secure(
        "http://127.0.0.1:7777/authorize",
        "http://127.0.0.1:7777/token",
        None,
        "http://127.0.0.1:7777",
    ));
    assert!(!secure(
        "https://auth.example.com/authorize",
        "http://auth.example.com/token",
        None,
        "https://auth.example.com",
    ));
    assert!(!secure(
        "http://auth.example.com/authorize",
        "https://auth.example.com/token",
        None,
        "https://auth.example.com",
    ));
    assert!(!secure(
        "https://auth.example.com/authorize",
        "https://auth.example.com/token",
        Some("http://auth.example.com/register"),
        "https://auth.example.com",
    ));
}

// Failure mode: a context that cannot open a browser waits for one anyway, or
// fails with a reason that does not tell the user what to do next.
// Owner layer: the MCP OAuth entry point.
#[tokio::test]
async fn a_non_interactive_context_refuses_the_browser_login() {
    let server = FakeAuthorizationServer::start(ProtectedResourceDocument::Wellformed).await;
    let error = authorize(
        IDENTITY,
        &server.mcp_url(),
        &McpOAuthConfig::default(),
        &header_map(&[]),
        McpAuthorizationMode::NonInteractive,
        Arc::new(MemoryCredentialStore::default()),
        no_browser(),
    )
    .await
    .expect_err("a browser login must not start here");

    let reported = format!("{error:#}");
    assert!(
        reported.contains("cannot open a browser"),
        "unexpected reason: {reported}"
    );
    assert!(
        reported.contains("Start Rho interactively"),
        "the error must say what to do instead: {reported}"
    );
    assert_eq!(
        server.token_requests().len(),
        0,
        "nothing may be exchanged without a login"
    );
}

// Failure mode: a broken or missing discovery document is read as "this server
// needs no authorization", so Rho invents endpoints or connects unauthorized.
// Owner layer: OAuth metadata discovery.
#[tokio::test]
async fn a_malformed_discovery_document_fails_with_a_reason() {
    let server = FakeAuthorizationServer::start(ProtectedResourceDocument::Malformed).await;
    let error = authorize(
        IDENTITY,
        &server.mcp_url(),
        &McpOAuthConfig::default(),
        &header_map(&[]),
        McpAuthorizationMode::Interactive,
        Arc::new(MemoryCredentialStore::default()),
        no_browser(),
    )
    .await
    .expect_err("a malformed document must not be treated as success");

    let reported = format!("{error:#}");
    assert!(
        reported.contains("published no OAuth metadata"),
        "unexpected reason: {reported}"
    );
}

// Failure mode: the 401 challenge is ignored, so discovery never finds the
// protected resource metadata, or the login runs without PKCE S256 and without
// the RFC 8707 resource indicator that binds the token to this server.
// Owner layer: the MCP OAuth login flow end to end.
#[tokio::test]
async fn the_browser_login_derives_an_s256_challenge_and_stores_the_token() {
    let server = FakeAuthorizationServer::start(ProtectedResourceDocument::Wellformed).await;
    let credentials = Arc::new(MemoryCredentialStore::default());
    let (prompt_sender, mut prompt_receiver) = tokio::sync::mpsc::unbounded_channel::<String>();

    let browser = tokio::spawn(async move {
        let authorize_url = prompt_receiver.recv().await.expect("an authorization URL");
        let parsed = url::Url::parse(&authorize_url).unwrap();
        let query: HashMap<String, String> = parsed
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        let redirect_uri = query.get("redirect_uri").expect("a redirect URI").clone();
        let state = query.get("state").expect("CSRF state").clone();
        // Stand in for the user approving the request.
        let redirect = url::Url::parse(&redirect_uri).unwrap();
        let callback = format!(
            "GET {}?code=granted-code&state={state} HTTP/1.1\r\nhost: loopback\r\n\r\n",
            redirect.path()
        );
        let mut stream = tokio::net::TcpStream::connect(format!(
            "{}:{}",
            redirect.host_str().unwrap(),
            redirect.port().unwrap()
        ))
        .await
        .unwrap();
        stream.write_all(callback.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        query
    });

    let client = authorize(
        IDENTITY,
        &server.mcp_url(),
        &McpOAuthConfig::default(),
        &header_map(&[]),
        McpAuthorizationMode::Interactive,
        credentials.clone(),
        AuthorizationPrompt::Captured(prompt_sender),
    )
    .await
    .expect("the login must succeed");

    let authorize_query = browser.await.unwrap();
    let token_request = server.token_requests().pop().expect("a token exchange");
    let token_form = form_fields(&token_request);

    assert_eq!(
        authorize_query
            .get("code_challenge_method")
            .map(String::as_str),
        Some("S256"),
        "PKCE must use S256"
    );
    assert_eq!(
        authorize_query.get("resource").map(String::as_str),
        Some(server.mcp_url().as_str()),
        "the RFC 8707 resource indicator must name this MCP server"
    );
    let verifier = token_form.get("code_verifier").expect("a PKCE verifier");
    let derived = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    assert_eq!(
        Some(&derived),
        authorize_query.get("code_challenge"),
        "the challenge must be BASE64URL(SHA256(verifier))"
    );
    assert_eq!(
        token_form.get("grant_type").map(String::as_str),
        Some("authorization_code")
    );
    assert_eq!(
        token_form.get("resource").map(String::as_str),
        Some(server.mcp_url().as_str())
    );
    assert_eq!(
        server.registrations().len(),
        1,
        "no client id was configured, so the client registers dynamically"
    );

    // The token is usable and persisted, so the next session does not log in
    // or register again.
    let McpHttpClient::Authorized(authorized) = client else {
        panic!("an OAuth server must produce an authorized client");
    };
    assert_eq!(
        authorized.get_access_token().await.unwrap(),
        "first-access-token"
    );
    let stored = credentials
        .get_secret(&store::account_name(IDENTITY))
        .unwrap()
        .expect("credentials are persisted");
    assert!(stored.contains("registered-client-id"));
}

// Failure mode: an expired access token is sent anyway, so every tool call
// fails with 401 until the user logs in again by hand.
// Owner layer: stored-credential reuse and refresh.
#[tokio::test]
async fn an_expired_access_token_is_refreshed_without_a_browser() {
    let server = FakeAuthorizationServer::start(ProtectedResourceDocument::Wellformed).await;
    let credentials = Arc::new(MemoryCredentialStore::default());
    let expired = serde_json::json!({
        "client_id": "registered-client-id",
        "token_response": {
            "access_token": "stale-access-token",
            "token_type": "bearer",
            "expires_in": 60,
            "refresh_token": "stored-refresh-token",
        },
        "granted_scopes": ["mcp"],
        // Issued long enough ago that the recorded lifetime has run out.
        "token_received_at": 1_700_000_000_u64,
        "issuer": serde_json::Value::Null,
    });
    credentials
        .set_secret(&store::account_name(IDENTITY), &expired.to_string())
        .unwrap();

    // No prompt is wired up: needing one here would fail the test rather than
    // silently open a browser.
    let client = authorize(
        IDENTITY,
        &server.mcp_url(),
        &McpOAuthConfig::default(),
        &header_map(&[]),
        McpAuthorizationMode::NonInteractive,
        credentials.clone(),
        no_browser(),
    )
    .await
    .expect("stored credentials must be reused");

    let McpHttpClient::Authorized(authorized) = client else {
        panic!("an OAuth server must produce an authorized client");
    };
    assert_eq!(
        authorized.get_access_token().await.unwrap(),
        "refreshed-access-token"
    );

    let token_request = server.token_requests().pop().expect("a refresh exchange");
    let token_form = form_fields(&token_request);
    assert_eq!(
        token_form.get("grant_type").map(String::as_str),
        Some("refresh_token")
    );
    assert_eq!(
        token_form.get("refresh_token").map(String::as_str),
        Some("stored-refresh-token")
    );
    assert_eq!(
        token_form.get("resource").map(String::as_str),
        Some(server.mcp_url().as_str()),
        "RFC 8707 binds the refreshed token to this server too"
    );
    assert_eq!(
        server.registrations().len(),
        0,
        "a stored registration must not be repeated"
    );
    let stored = credentials
        .get_secret(&store::account_name(IDENTITY))
        .unwrap()
        .expect("the refreshed token is persisted");
    assert!(stored.contains("refreshed-access-token"));
}

// Failure mode: a token reaches the config inventory or `rho mcp list --json`,
// where it lands in logs, bug reports, and shared terminals.
// Owner layer: MCP config and session-report serialization.
#[test]
fn tokens_never_reach_the_serialized_inventory() {
    let toml = r#"
transport = "streamable_http"
url = "https://mcp.example.com/mcp"

[oauth]
client_id = "configured-client"
scopes = ["mcp"]
"#;
    let server: crate::tools::mcp::config::McpServerConfig = toml::from_str(toml).unwrap();
    let report = crate::tools::mcp::McpServerReport::disabled(IDENTITY.to_string(), &server);

    let serialized = format!(
        "{}{}",
        serde_json::to_string(&server).unwrap(),
        serde_json::to_string(&report).unwrap()
    );

    for secret in [
        "access_token",
        "refresh_token",
        "Bearer",
        "first-access-token",
        "client_secret",
    ] {
        assert!(
            !serialized.contains(secret),
            "`{secret}` must not appear in the inventory: {serialized}"
        );
    }
    // The non-secret opt-in still shows, so `rho mcp show` can explain itself.
    assert!(serialized.contains("configured-client"));
}

// Failure mode: a misspelled or unusable `oauth` key is accepted quietly, so
// the first sign of trouble is a bare error from the token endpoint.
// Owner layer: the MCP configuration parser.
#[test]
fn the_oauth_table_is_parsed_strictly() {
    let parse = |table: &str| {
        toml::from_str::<crate::tools::mcp::config::McpServerConfig>(&format!(
            "transport = \"streamable_http\"\nurl = \"https://mcp.example.com/mcp\"\n{table}"
        ))
    };

    let accepted = parse("[oauth]\nclient_id = \"c\"\nscopes = [\"a\", \"b\"]\n").unwrap();
    let crate::tools::mcp::config::McpTransport::StreamableHttp { oauth, .. } = accepted.transport
    else {
        panic!("expected a streamable-http transport");
    };
    assert_eq!(
        oauth,
        Some(McpOAuthConfig {
            client_id: Some("c".into()),
            scopes: vec!["a".into(), "b".into()],
        })
    );

    // An empty table is the bare opt-in.
    assert!(parse("[oauth]\n").is_ok());
    // No table at all leaves the server unauthorized.
    assert!(parse("").is_ok());

    for rejected in [
        "[oauth]\nclientid = \"c\"\n",
        "[oauth]\nclient_secret = \"s\"\n",
        "[oauth]\nclient_id = \"  \"\n",
        "[oauth]\nscopes = [\"\"]\n",
        "[oauth]\nscopes = [\"read write\"]\n",
        "[oauth]\nscopes = [\"mcp \"]\n",
        "[oauth]\nscopes = [\" mcp\"]\n",
    ] {
        assert!(parse(rejected).is_err(), "must reject: {rejected}");
    }
}

/// Decode an `application/x-www-form-urlencoded` request body.
fn form_fields(body: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

/// Whether the protected resource metadata document parses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProtectedResourceDocument {
    Wellformed,
    Malformed,
}

#[derive(Default)]
struct ServerRecord {
    token_requests: Vec<String>,
    registrations: Vec<String>,
}

/// A loopback MCP endpoint that also plays its own authorization server.
///
/// Real sockets rather than mocks, so the discovery, registration, and token
/// requests under test are the ones Rho actually sends.
struct FakeAuthorizationServer {
    address: SocketAddr,
    record: Arc<Mutex<ServerRecord>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeAuthorizationServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeAuthorizationServer {
    async fn start(document: ProtectedResourceDocument) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let record = Arc::new(Mutex::new(ServerRecord::default()));
        let task = tokio::spawn({
            let record = Arc::clone(&record);
            async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let record = Arc::clone(&record);
                    tokio::spawn(async move {
                        let Some((method, target, body)) = read_request(&mut stream).await else {
                            return;
                        };
                        let response = route(&method, &target, &body, address, document, &record);
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            }
        });
        Self {
            address,
            record,
            task,
        }
    }

    fn mcp_url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }

    fn token_requests(&self) -> Vec<String> {
        self.record.lock().unwrap().token_requests.clone()
    }

    fn registrations(&self) -> Vec<String> {
        self.record.lock().unwrap().registrations.clone()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, String, String)> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    let headers_end = loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
    };
    let headers = String::from_utf8_lossy(&request[..headers_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let body_start = headers_end + 4;
    while request.len() < body_start + content_length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let mut request_line = headers
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let body = String::from_utf8_lossy(&request[body_start..]).into_owned();
    Some((method, target, body))
}

fn route(
    method: &str,
    target: &str,
    body: &str,
    address: SocketAddr,
    document: ProtectedResourceDocument,
    record: &Mutex<ServerRecord>,
) -> String {
    let path = target.split('?').next().unwrap_or(target);
    let origin = format!("http://{address}");
    match (method, path) {
        // The unauthenticated probe: a 401 pointing at the protected resource
        // metadata document, exactly as the authorization spec describes.
        ("POST", "/mcp") => format!(
            "HTTP/1.1 401 Unauthorized\r\nwww-authenticate: Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource\", scope=\"mcp\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        ),
        ("GET", "/.well-known/oauth-protected-resource")
        | ("GET", "/.well-known/oauth-protected-resource/mcp") => match document {
            ProtectedResourceDocument::Wellformed => json_response(&serde_json::json!({
                "resource": format!("{origin}/mcp"),
                "authorization_servers": [origin],
                "scopes_supported": ["mcp"],
            })),
            ProtectedResourceDocument::Malformed => text_response("200 OK", "{ \"resource\": "),
        },
        // A malformed protected resource document is the only lead this server
        // gives, so nothing else answers either.
        ("GET", "/.well-known/oauth-authorization-server")
            if document == ProtectedResourceDocument::Malformed =>
        {
            text_response("404 Not Found", "no such endpoint")
        }
        ("GET", "/.well-known/oauth-authorization-server") => json_response(&serde_json::json!({
            "issuer": origin,
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "registration_endpoint": format!("{origin}/register"),
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
            "scopes_supported": ["mcp"],
        })),
        ("POST", "/register") => {
            record.lock().unwrap().registrations.push(body.to_string());
            json_response(&serde_json::json!({
                "client_id": "registered-client-id",
                "redirect_uris": [],
            }))
        }
        ("POST", "/token") => {
            record.lock().unwrap().token_requests.push(body.to_string());
            let refreshing = body.contains("grant_type=refresh_token");
            json_response(&serde_json::json!({
                "access_token": if refreshing { "refreshed-access-token" } else { "first-access-token" },
                "token_type": "bearer",
                "expires_in": 3600,
                "refresh_token": "next-refresh-token",
                "scope": "mcp",
            }))
        }
        _ => text_response("404 Not Found", "no such endpoint"),
    }
}

fn json_response(body: &serde_json::Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn text_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}
