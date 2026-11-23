//! The webhook targets `rho mcp serve --http` POSTs a watched session's
//! news to, kept in rho's credential store beside the tokens: one entry
//! holds every target's name, URL, secret, and the tokens bound to it.
//!
//! A caller only names a target. Its URL and secret come from the owner's
//! `rho mcp webhook add`, so a caller cannot aim the server's requests
//! anywhere (no SSRF guard is needed, as for configured MCP and OpenAPI
//! URLs), and a token may use only the targets bound to it. The secret is
//! never shown, logged, or returned: `Debug` leaves it out.

use std::fmt;

use rho_providers::credentials::CredentialStore;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use super::token_store::{self, StoredSet};
use crate::app::http_guard::validate_name;

/// The credential-store account that holds the targets.
pub(crate) const ACCOUNT: &str = "mcp-serve-webhooks";

/// Every webhook target, saved as a JSON array.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct WebhookSet {
    pub(crate) targets: Vec<WebhookTarget>,
}

/// One target: where to POST, the bearer secret to send, and the tokens
/// whose watches may use it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WebhookTarget {
    pub(crate) name: String,
    /// An `https` URL, or `http` to a loopback host; no userinfo.
    pub(crate) url: String,
    /// Sent as `Authorization: Bearer <secret>`.
    pub(crate) secret: String,
    /// Names of the tokens that may use the target; at least one.
    pub(crate) tokens: Vec<String>,
}

impl fmt::Debug for WebhookTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookTarget")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("tokens", &self.tokens)
            .finish_non_exhaustive()
    }
}

/// A token asked for a target not bound to it, or none of that name. The
/// two read the same, so a token learns nothing of other tokens' targets.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "token {token:?} may not use webhook {webhook:?}; it may use: {}",
    list(usable)
)]
pub(crate) struct Unbound {
    pub(crate) token: String,
    pub(crate) webhook: String,
    /// The targets bound to the token.
    pub(crate) usable: Vec<String>,
}

fn list(names: &[String]) -> String {
    if names.is_empty() {
        return "(none)".to_owned();
    }
    names.join(", ")
}

impl StoredSet for WebhookSet {
    const ACCOUNT: &'static str = ACCOUNT;
    const WHAT: &'static str = "MCP server webhooks";
}

impl WebhookSet {
    /// The set saved in `store`; empty when none is.
    pub(crate) fn load(store: &dyn CredentialStore) -> anyhow::Result<Self> {
        token_store::load(store)
    }

    /// Adds target `name`, or replaces its URL, secret, and tokens.
    pub(crate) fn upsert(
        &mut self,
        name: &str,
        url: &str,
        secret: &str,
        tokens: &[String],
    ) -> anyhow::Result<()> {
        validate_name("webhook", name)?;
        let url = validate_url(url)?;
        let secret = secret.trim();
        if secret.is_empty() || secret.chars().any(char::is_control) {
            anyhow::bail!("the webhook secret must be one non-empty line");
        }
        if tokens.is_empty() {
            anyhow::bail!(
                "a webhook needs at least one --token: the tokens whose watches may use it"
            );
        }
        let mut bound: Vec<String> = Vec::new();
        for token in tokens {
            validate_name("token", token)?;
            if !bound.contains(token) {
                bound.push(token.clone());
            }
        }
        let target = WebhookTarget {
            name: name.to_owned(),
            url: url.to_string(),
            secret: secret.to_owned(),
            tokens: bound,
        };
        match self.targets.iter_mut().find(|target| target.name == name) {
            Some(existing) => *existing = target,
            None => self.targets.push(target),
        }
        Ok(())
    }

    /// Removes target `name`; whether it existed.
    pub(crate) fn remove(&mut self, name: &str) -> bool {
        let before = self.targets.len();
        self.targets.retain(|target| target.name != name);
        self.targets.len() != before
    }

    /// Target `name`, if it is bound to `token`.
    pub(crate) fn for_token(&self, token: &str, name: &str) -> Result<&WebhookTarget, Unbound> {
        let bound = |target: &&WebhookTarget| target.tokens.iter().any(|bound| bound == token);
        self.targets
            .iter()
            .filter(bound)
            .find(|target| target.name == name)
            .ok_or_else(|| Unbound {
                token: token.to_owned(),
                webhook: name.to_owned(),
                usable: self
                    .targets
                    .iter()
                    .filter(bound)
                    .map(|target| target.name.clone())
                    .collect(),
            })
    }
}

/// `url` as a webhook may use it: `https`, or `http` only to a loopback
/// host, which no network between can read, and no userinfo, which would
/// put a second secret in the URL every list shows.
pub(crate) fn validate_url(url: &str) -> anyhow::Result<Url> {
    let parsed =
        Url::parse(url).map_err(|error| anyhow::anyhow!("webhook URL {url:?}: {error}"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        // Not echoed: the userinfo may be a secret.
        anyhow::bail!(
            "the webhook URL carries a user name or password; pipe the secret on stdin instead"
        );
    }
    let loopback = match parsed.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => anyhow::bail!("webhook URL {url:?} names no host"),
    };
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if loopback => Ok(parsed),
        "http" => anyhow::bail!(
            "webhook URL {url:?} must use https; plain http is allowed only to a loopback host"
        ),
        scheme => anyhow::bail!("webhook URL {url:?} must use https, not {scheme}"),
    }
}

#[cfg(test)]
#[path = "webhooks_tests.rs"]
mod tests;
