//! Configuration validation shared by the MCP config parser and the CLI.
//!
//! Every check here runs before a transport, process, or request exists, so a
//! malformed entry fails at parse time rather than at session start.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;

use anyhow::{bail, Context};
use http::{HeaderName, HeaderValue};

pub(crate) fn validate_identity(identity: &str) -> anyhow::Result<()> {
    if identity.is_empty()
        || !identity
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("server identity must contain only ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

/// Transport security for a remote MCP URL.
///
/// OAuth discovery endpoints always use [`Self::HttpsOrLoopback`]. A user
/// config entry may opt a Streamable HTTP server into cleartext HTTP; plugin
/// manifests cannot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum McpHttpSecurity {
    /// HTTPS, or plain HTTP only when the host is loopback.
    #[default]
    HttpsOrLoopback,
    /// User opted this server into cleartext HTTP, including non-loopback
    /// hosts. Headers and bearer tokens then travel unencrypted.
    AllowInsecureHttp,
}

impl McpHttpSecurity {
    pub(crate) const fn from_allow_insecure_http(allow: bool) -> Self {
        if allow {
            Self::AllowInsecureHttp
        } else {
            Self::HttpsOrLoopback
        }
    }
}

pub(crate) fn parse_remote_url(value: &str) -> anyhow::Result<url::Url> {
    parse_remote_url_with(value, McpHttpSecurity::HttpsOrLoopback)
}

pub(crate) fn parse_remote_url_with(
    value: &str,
    security: McpHttpSecurity,
) -> anyhow::Result<url::Url> {
    parse_transport_secure_url(value, "remote MCP URL", security)
        .context("invalid Streamable HTTP URL")
}

/// Apply the `parse_remote_url` transport rule to an OAuth endpoint taken from
/// a discovery document, so a server cannot downgrade the login to plaintext.
/// `purpose` names the endpoint in the error, such as `token endpoint`.
pub(crate) fn parse_oauth_endpoint(value: &str, purpose: &str) -> anyhow::Result<url::Url> {
    parse_transport_secure_url(
        value,
        &format!("OAuth {purpose}"),
        McpHttpSecurity::HttpsOrLoopback,
    )
    .with_context(|| format!("invalid OAuth {purpose}"))
}

/// One transport-security rule for every URL Rho talks to: HTTPS, or plain
/// HTTP only when the host is loopback, unless the user opted this server into
/// cleartext HTTP.
fn parse_transport_secure_url(
    value: &str,
    subject: &str,
    security: McpHttpSecurity,
) -> anyhow::Result<url::Url> {
    let url = url::Url::parse(value)?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => bail!("{subject} must have a host"),
    };
    let cleartext_ok =
        url.scheme() == "http" && (loopback || security == McpHttpSecurity::AllowInsecureHttp);
    if url.scheme() != "https" && !cleartext_ok {
        match security {
            McpHttpSecurity::HttpsOrLoopback => {
                bail!("{subject} must use HTTPS unless its host is loopback")
            }
            McpHttpSecurity::AllowInsecureHttp => {
                bail!("{subject} must use HTTP or HTTPS")
            }
        }
    }
    Ok(url)
}

/// Reject OAuth client material that could only fail later, at the token
/// endpoint, where the reason would be a bare server error.
pub(crate) fn validate_oauth_client(
    client_id: Option<&str>,
    scopes: &[String],
) -> anyhow::Result<()> {
    if client_id.is_some_and(|client_id| client_id.trim().is_empty()) {
        bail!("MCP oauth client_id must not be empty");
    }
    for scope in scopes {
        if scope.trim().is_empty() {
            bail!("MCP oauth scopes must not contain an empty entry");
        }
        if scope.chars().any(char::is_whitespace) {
            bail!("MCP oauth scope `{scope}` must not contain whitespace; list scopes separately");
        }
    }
    Ok(())
}

pub(crate) fn validate_literal_headers(headers: &BTreeMap<String, String>) -> anyhow::Result<()> {
    validate_header_names(headers.keys())?;
    for value in headers.values() {
        HeaderValue::try_from(value).context("invalid MCP header value")?;
    }
    Ok(())
}

pub(crate) fn validate_environment_header_names(
    headers: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    validate_header_names(headers.keys())
}

/// Reject blank/invalid env names, NULs, and duplicate child variable names
/// across `env` and `env_from_env` before a stdio server is constructed.
pub(crate) fn validate_stdio_environment(
    env: &BTreeMap<String, String>,
    env_from_env: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let mut child_names = HashSet::new();
    for (name, value) in env {
        validate_process_env_name(name, "env")?;
        validate_process_env_value(value, name)?;
        if !child_names.insert(name.as_str()) {
            bail!("stdio env repeats child variable `{name}`");
        }
    }
    for (name, source) in env_from_env {
        validate_process_env_name(name, "env_from_env")?;
        validate_process_env_name(source, "env_from_env source")?;
        if !child_names.insert(name.as_str()) {
            bail!("stdio env repeats child variable `{name}` across env and env_from_env");
        }
    }
    Ok(())
}

fn validate_process_env_name(name: &str, field: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        bail!("stdio {field} variable name must not be empty");
    }
    if name.contains('=') {
        bail!("stdio {field} variable name `{name}` must not contain '='");
    }
    if name.contains('\0') {
        bail!("stdio {field} variable name must not contain NUL");
    }
    Ok(())
}

fn validate_process_env_value(value: &str, name: &str) -> anyhow::Result<()> {
    if value.contains('\0') {
        bail!("stdio env value for `{name}` must not contain NUL");
    }
    Ok(())
}

fn validate_header_names<'a>(names: impl IntoIterator<Item = &'a String>) -> anyhow::Result<()> {
    let mut parsed = HashSet::new();
    for name in names {
        let name = HeaderName::try_from(name).context("invalid MCP header name")?;
        if !parsed.insert(name) {
            bail!("MCP headers repeat a name under different casing");
        }
    }
    Ok(())
}
