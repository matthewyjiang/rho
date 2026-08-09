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

pub(crate) fn parse_remote_url(value: &str) -> anyhow::Result<url::Url> {
    let url = url::Url::parse(value).context("invalid Streamable HTTP URL")?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => bail!("remote MCP URL must have a host"),
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("remote MCP URL must use HTTPS unless its host is loopback");
    }
    Ok(url)
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
