//! `servers` lists, resolved to absolute HTTP(S) URLs.

use serde_json::{Map, Value};
use url::Url;

/// Why one `servers` entry has no usable URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ServerUrlError {
    #[error("server entry has no url")]
    MissingUrl,
    #[error("server URL '{url}' uses variable '{name}', which has no default")]
    MissingVariable { url: String, name: String },
    #[error("server URL '{url}' has an unclosed variable")]
    UnclosedVariable { url: String },
    #[error("server URL '{url}' is invalid: {reason}")]
    Invalid { url: String, reason: String },
    #[error("server URL '{url}' does not resolve to an HTTP(S) URL")]
    NotHttp { url: String },
    #[error("server URL '{url}' has a query or fragment")]
    QueryOrFragment { url: String },
}

/// Resolved `servers` entries, in spec order.
pub(crate) type Servers = Vec<Result<Url, ServerUrlError>>;

/// The spec's top-level servers. A spec without servers means `/`, per OpenAPI.
pub(super) fn resolve_spec(root: &Value, spec_url: &Url) -> Servers {
    root.as_object()
        .and_then(|root| resolve_declared(root, spec_url))
        .unwrap_or_else(|| vec![resolve_url("/", spec_url)])
}

/// Resolve the `servers` declared on `owner` (the spec root, a path item, or an
/// operation) against `spec_url`, filling variables with their defaults.
/// Returns `None` when `owner` declares none.
pub(super) fn resolve_declared(owner: &Map<String, Value>, spec_url: &Url) -> Option<Servers> {
    let servers = owner.get("servers")?.as_array()?;
    if servers.is_empty() {
        return None;
    }
    Some(
        servers
            .iter()
            .map(|server| resolve_one(server, spec_url))
            .collect(),
    )
}

fn resolve_one(server: &Value, spec_url: &Url) -> Result<Url, ServerUrlError> {
    let template = server
        .get("url")
        .and_then(Value::as_str)
        .ok_or(ServerUrlError::MissingUrl)?;
    let mut url = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        url.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| ServerUrlError::UnclosedVariable {
                url: template.to_owned(),
            })?;
        let name = &after[..close];
        // OpenAPI requires a string, but YAML reads an unquoted `8443` as a number.
        let default = match server
            .get("variables")
            .and_then(|variables| variables.get(name))
            .and_then(|variable| variable.get("default"))
        {
            Some(Value::String(default)) => default.clone(),
            Some(Value::Number(default)) => default.to_string(),
            _ => {
                return Err(ServerUrlError::MissingVariable {
                    url: template.to_owned(),
                    name: name.to_owned(),
                })
            }
        };
        url.push_str(&default);
        rest = &after[close + 1..];
    }
    url.push_str(rest);
    resolve_url(&url, spec_url)
}

fn resolve_url(url: &str, spec_url: &Url) -> Result<Url, ServerUrlError> {
    let resolved = spec_url
        .join(url)
        .map_err(|error| ServerUrlError::Invalid {
            url: url.to_owned(),
            reason: error.to_string(),
        })?;
    if !matches!(resolved.scheme(), "http" | "https") {
        return Err(ServerUrlError::NotHttp {
            url: resolved.to_string(),
        });
    }
    // Tools append operation paths and their own query strings.
    if resolved.query().is_some() || resolved.fragment().is_some() {
        return Err(ServerUrlError::QueryOrFragment {
            url: resolved.to_string(),
        });
    }
    Ok(resolved)
}
