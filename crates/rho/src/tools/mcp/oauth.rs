//! OAuth 2.1 authorization for Streamable HTTP MCP servers.
//!
//! rmcp owns the protocol: RFC 9728 protected resource metadata, RFC 8414
//! authorization server metadata, RFC 7591 dynamic client registration,
//! PKCE with S256, the RFC 8707 `resource` indicator, issuer binding, and
//! refresh. This module owns the parts that are Rho's: when authorization
//! applies at all, where the tokens live, whether a browser may be opened,
//! and the budgets that keep a stalled login from hanging session startup.

use std::{collections::HashMap, sync::Arc};

use anyhow::{bail, Context};
use http::{HeaderName, HeaderValue};
use rmcp::transport::auth::{
    AuthClient, AuthorizationManager, AuthorizationMetadata, AuthorizationMetadataSource,
    AuthorizationRequest, AuthorizationSession,
};

use super::{config::McpOAuthConfig, validate};

pub(crate) mod callback;
pub(crate) mod store;

/// Client name offered during dynamic client registration.
const CLIENT_NAME: &str = "Rho";
/// Discovery, stored-credential load, and refresh are machine-to-machine work.
/// A minute is generous for them and still bounds an unresponsive server.
const DISCOVERY_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);
/// A browser login involves a person. Five minutes is the usual grant window;
/// past that the session start fails rather than waiting forever.
const LOGIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(300);

/// Whether this process may hand the user a browser login.
///
/// Session startup blocks on the answer, so it is decided by the caller that
/// knows the context rather than sniffed deep in the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpAuthorizationMode {
    /// A person is at the terminal and can finish a browser login.
    Interactive,
    /// Batch, CI, or inventory context: refuse rather than block.
    NonInteractive,
}

/// Whether the process still has a terminal on both ends of the conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalAttachment {
    Attached,
    Detached,
}

/// Whether the process looks like an automated runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CiEnvironment {
    Detected,
    Absent,
}

impl McpAuthorizationMode {
    /// Decide from injected facts so the rule stays testable.
    ///
    /// CI runners often keep a pseudo-terminal, so an explicit CI marker
    /// refuses on its own.
    pub(crate) fn resolve(terminal: TerminalAttachment, ci: CiEnvironment) -> Self {
        match (terminal, ci) {
            (TerminalAttachment::Attached, CiEnvironment::Absent) => Self::Interactive,
            (TerminalAttachment::Detached, _) | (_, CiEnvironment::Detected) => {
                Self::NonInteractive
            }
        }
    }

    /// Read the facts from the running process, at the edge.
    pub(crate) fn from_process() -> Self {
        use std::io::IsTerminal;

        let terminal = if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
            TerminalAttachment::Attached
        } else {
            TerminalAttachment::Detached
        };
        let ci = match std::env::var("CI") {
            Ok(value) if !value.trim().is_empty() && value != "0" && value != "false" => {
                CiEnvironment::Detected
            }
            Ok(_) | Err(_) => CiEnvironment::Absent,
        };
        Self::resolve(terminal, ci)
    }
}

/// The HTTP client a Streamable HTTP session should run on.
pub(super) enum McpHttpClient {
    /// rmcp's own client. Configured headers alone carry any credential.
    Default,
    /// An OAuth-bearing client that attaches the access token and refreshes it
    /// when it expires.
    Authorized(Box<AuthClient<reqwest::Client>>),
}

/// Names the variant only. The authorized client holds live credentials, and
/// nothing that holds credentials gets a derived `Debug`.
impl std::fmt::Debug for McpHttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => formatter.write_str("McpHttpClient::Default"),
            Self::Authorized(_) => formatter.write_str("McpHttpClient::Authorized"),
        }
    }
}

/// How the authorization URL reaches the person who must approve it.
///
/// Production opens the desktop browser. Tests capture the URL so the flow can
/// run end to end without launching one.
enum AuthorizationPrompt {
    DesktopBrowser,
    #[cfg(test)]
    Captured(tokio::sync::mpsc::UnboundedSender<String>),
}

impl AuthorizationPrompt {
    fn present(&self, url: &str) -> anyhow::Result<()> {
        match self {
            Self::DesktopBrowser => webbrowser::open(url)
                .map(|_| ())
                .context("could not open a browser for the MCP authorization login"),
            #[cfg(test)]
            Self::Captured(sender) => sender
                .send(url.to_string())
                .context("authorization URL had no reader"),
        }
    }
}

/// Whether OAuth applies to one Streamable HTTP server.
#[derive(Debug, PartialEq, Eq)]
enum AuthorizationPlan<'a> {
    /// No `oauth` table, or the user already named the credential to send.
    Skip,
    Run(&'a McpOAuthConfig),
}

/// Decide whether to authorize, and say why when the answer is no.
///
/// A configured `Authorization` header wins outright: the user told Rho which
/// credential to use, and starting a browser login behind their back would
/// mint a second one they never asked for.
fn authorization_plan<'a>(
    identity: &str,
    oauth: Option<&'a McpOAuthConfig>,
    headers: &HashMap<HeaderName, HeaderValue>,
) -> AuthorizationPlan<'a> {
    let Some(oauth) = oauth else {
        return AuthorizationPlan::Skip;
    };
    if headers.contains_key(&http::header::AUTHORIZATION) {
        tracing::info!(
            server = %identity,
            "MCP server has a configured Authorization header; its OAuth configuration is unused"
        );
        return AuthorizationPlan::Skip;
    }
    AuthorizationPlan::Run(oauth)
}

/// Resolve the HTTP client for one Streamable HTTP server before its session
/// starts, running the OAuth flow when the server is configured for it.
pub(super) async fn prepare_http_client(
    identity: &str,
    url: &str,
    oauth: Option<&McpOAuthConfig>,
    headers: &HashMap<HeaderName, HeaderValue>,
    mode: McpAuthorizationMode,
) -> anyhow::Result<McpHttpClient> {
    let oauth = match authorization_plan(identity, oauth, headers) {
        AuthorizationPlan::Skip => return Ok(McpHttpClient::Default),
        AuthorizationPlan::Run(oauth) => oauth,
    };
    authorize(
        identity,
        url,
        oauth,
        headers,
        mode,
        Arc::new(crate::credential_store::AppCredentialStore),
        AuthorizationPrompt::DesktopBrowser,
    )
    .await
    .with_context(|| format!("MCP server `{identity}` could not be authorized"))
}

/// Drive discovery, reuse, and login for one server.
///
/// Split from `prepare_http_client` so tests can supply their own credential
/// store and browser stand-in instead of the process-wide ones.
async fn authorize(
    identity: &str,
    url: &str,
    oauth: &McpOAuthConfig,
    headers: &HashMap<HeaderName, HeaderValue>,
    mode: McpAuthorizationMode,
    credentials: Arc<dyn rho_providers::credentials::CredentialStore>,
    prompt: AuthorizationPrompt,
) -> anyhow::Result<McpHttpClient> {
    let store = store::McpOAuthCredentialStore::new(identity, credentials);
    let mut manager = tokio::time::timeout(
        DISCOVERY_BUDGET,
        discover(identity, url, headers, store.clone()),
    )
    .await
    .with_context(|| {
        format!(
            "OAuth discovery exceeded {} seconds",
            DISCOVERY_BUDGET.as_secs()
        )
    })??;

    let reusable = tokio::time::timeout(DISCOVERY_BUDGET, manager.initialize_from_store())
        .await
        .with_context(|| {
            format!(
                "reading stored OAuth credentials exceeded {} seconds",
                DISCOVERY_BUDGET.as_secs()
            )
        })?
        .context("stored OAuth credentials could not be reused")?;
    if reusable {
        tracing::debug!(server = %identity, "reusing stored MCP OAuth credentials");
        return authorized_client(manager);
    }

    match mode {
        McpAuthorizationMode::Interactive => {}
        McpAuthorizationMode::NonInteractive => bail!(
            "no stored OAuth credentials and this context cannot open a browser. \
             Start Rho interactively in a terminal to authorize `{identity}`, \
             then rerun this command."
        ),
    }

    let manager = tokio::time::timeout(LOGIN_BUDGET, log_in(identity, manager, oauth, prompt))
        .await
        .with_context(|| {
            format!(
                "browser authorization exceeded {} seconds",
                LOGIN_BUDGET.as_secs()
            )
        })??;
    authorized_client(manager)
}

fn authorized_client(manager: AuthorizationManager) -> anyhow::Result<McpHttpClient> {
    Ok(McpHttpClient::Authorized(Box::new(AuthClient::new(
        transport_http_client()?,
        manager,
    ))))
}

/// Match the transport client rmcp builds for itself: no automatic redirects,
/// so headers and bearer tokens cannot be replayed to a redirect target, and
/// no idle pooling, which stalls on Linux delayed ACK.
fn transport_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("reqwest client with static settings could not build")
}

/// Learn where to authorize: ask the server, read its 401 challenge, then
/// resolve and vet the authorization server metadata.
async fn discover(
    identity: &str,
    url: &str,
    headers: &HashMap<HeaderName, HeaderValue>,
    store: store::McpOAuthCredentialStore,
) -> anyhow::Result<AuthorizationManager> {
    let mut manager = AuthorizationManager::new(url)
        .await
        .context("could not start OAuth discovery")?;
    manager.set_credential_store(store);

    let challenge = protected_resource_challenge(url, headers).await;
    let resolution = manager
        .resolve_metadata_from_challenge(challenge.as_deref())
        .await
        .context("authorization server metadata could not be read")?;
    match resolution.source {
        AuthorizationMetadataSource::ProtectedResourceMetadata
        | AuthorizationMetadataSource::AuthorizationServerMetadata => {}
        AuthorizationMetadataSource::LegacyEndpointFallback => bail!(
            "server published no OAuth metadata; \
             Rho does not guess authorization endpoints from the MCP URL"
        ),
        // rmcp marks the enum non-exhaustive. A discovery route Rho has not
        // vetted is refused rather than trusted by default.
        source => bail!("OAuth metadata came from an unrecognized source: {source:?}"),
    }
    validate_metadata(&resolution.metadata)?;
    tracing::debug!(
        server = %identity,
        issuer = resolution.metadata.issuer.as_deref().unwrap_or("-"),
        "resolved MCP authorization server metadata"
    );
    manager.set_metadata(resolution.metadata);
    Ok(manager)
}

/// Every OAuth endpoint must clear the same bar as the MCP URL itself.
///
/// rmcp binds the metadata to its issuer during discovery; this refuses a
/// document that would move any leg of the exchange onto plaintext HTTP.
fn validate_metadata(metadata: &AuthorizationMetadata) -> anyhow::Result<()> {
    validate::parse_oauth_endpoint(&metadata.authorization_endpoint, "authorization endpoint")?;
    validate::parse_oauth_endpoint(&metadata.token_endpoint, "token endpoint")?;
    if let Some(registration_endpoint) = &metadata.registration_endpoint {
        validate::parse_oauth_endpoint(registration_endpoint, "registration endpoint")?;
    }
    if let Some(issuer) = &metadata.issuer {
        validate::parse_oauth_endpoint(issuer, "issuer")?;
    }
    Ok(())
}

/// Ask the MCP endpoint one unauthenticated question and keep the
/// `WWW-Authenticate` challenge from its 401.
///
/// The challenge names the protected resource metadata document and the scopes
/// the operation needs, which is what the authorization spec asks clients to
/// discover from. A server that answers anything else leaves discovery to the
/// well-known paths.
async fn protected_resource_challenge(
    url: &str,
    headers: &HashMap<HeaderName, HeaderValue>,
) -> Option<String> {
    let client = match transport_http_client() {
        Ok(client) => client,
        Err(error) => {
            tracing::debug!(
                error = %error,
                "MCP authorization HTTP client could not be built; falling back to well-known discovery"
            );
            return None;
        }
    };
    let mut request = client
        .post(url)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::ACCEPT, "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "ping",
        }));
    for (name, value) in headers {
        request = request.header(name.clone(), value.clone());
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(error = %error, "MCP authorization probe failed; falling back to well-known discovery");
            return None;
        }
    };
    if response.status() != http::StatusCode::UNAUTHORIZED {
        return None;
    }
    response
        .headers()
        .get(http::header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Run the authorization code flow with PKCE through the user's browser.
///
/// The loopback listener binds first so the redirect URI registered with the
/// authorization server is the one Rho is already listening on.
async fn log_in(
    identity: &str,
    manager: AuthorizationManager,
    oauth: &McpOAuthConfig,
    prompt: AuthorizationPrompt,
) -> anyhow::Result<AuthorizationManager> {
    let redirect = callback::LoopbackRedirect::bind().await?;
    let mut request =
        AuthorizationRequest::new(redirect.redirect_uri()).with_client_name(CLIENT_NAME);
    if let Some(client_id) = &oauth.client_id {
        request = request.with_preregistered_client(client_id.clone());
    }
    if !oauth.scopes.is_empty() {
        request = request.with_scopes(oauth.scopes.clone());
    }

    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|(_, error)| anyhow::anyhow!(error))
        .context("authorization request could not be prepared")?;

    let auth_url = session.get_authorization_url();
    let auth_origin = url::Url::parse(auth_url)
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or_else(|_| "unknown".to_string());
    tracing::info!(
        server = %identity,
        origin = %auth_origin,
        "opening a browser to authorize this MCP server"
    );
    prompt.present(auth_url)?;

    // Validate CSRF state on the loopback acceptor so a mismatched callback
    // cannot stop the listener before the real browser redirect arrives.
    let expected_state = callback::state_from_authorization_url(auth_url)?;
    let redirected_to = redirect.wait_for_redirect(&expected_state).await?;
    session
        .handle_callback_url(&redirected_to)
        .await
        .map_err(|error| anyhow::anyhow!(error))
        .context("authorization code could not be exchanged for a token")?;
    Ok(session.auth_manager)
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
