//! The remotes a hub `rho mcp serve` reaches, kept in rho's credential
//! store beside the tokens and webhooks: one entry holds every remote's
//! name, MCP URL, and the bearer secret of the token the remote issued the
//! hub. `Debug` leaves the secret out; nothing lists, logs, or returns it.

use std::{fmt, net::SocketAddr};

use rho_providers::credentials::CredentialStore;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::app::{
    http_guard::{is_private, validate_name},
    mcp_server::token_store::{self, StoredSet},
};

/// The credential-store account that holds the remotes.
const ACCOUNT: &str = "mcp-serve-remotes";

/// Every remote, saved as a JSON array.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RemoteSet {
    pub(crate) remotes: Vec<Remote>,
}

/// One remote `rho mcp serve --http`: its name, which calls give as
/// `server`, where it answers, and the secret the hub sends it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Remote {
    pub(crate) name: String,
    /// Its MCP endpoint, as [`check_url`] passed it.
    pub(crate) url: String,
    /// Sent as `Authorization: Bearer <secret>`.
    pub(crate) secret: String,
}

impl Remote {
    /// Remote `name` at `url`, one [`check_url`] passed, with `secret`.
    pub(crate) fn new(name: &str, url: Url, secret: &str) -> anyhow::Result<Self> {
        check_name(name)?;
        let secret = secret.trim();
        if secret.is_empty() || secret.chars().any(char::is_control) {
            anyhow::bail!("the remote's token secret must be one non-empty line");
        }
        Ok(Self {
            name: name.to_owned(),
            url: url.to_string(),
            secret: secret.to_owned(),
        })
    }
}

/// Refuses a remote name that `server` and refusals could not show plainly.
pub(crate) fn check_name(name: &str) -> anyhow::Result<()> {
    validate_name("remote", name)
}

impl fmt::Debug for Remote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Remote")
            .field("name", &self.name)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl StoredSet for RemoteSet {
    const ACCOUNT: &'static str = ACCOUNT;
    const WHAT: &'static str = "MCP server remotes";
}

impl RemoteSet {
    /// The set saved in `store`; empty when none is.
    pub(crate) fn load(store: &dyn CredentialStore) -> anyhow::Result<Self> {
        token_store::load(store)
    }

    /// Adds `remote`, or replaces the one of its name.
    pub(crate) fn upsert(&mut self, remote: Remote) {
        match self
            .remotes
            .iter_mut()
            .find(|saved| saved.name == remote.name)
        {
            Some(existing) => *existing = remote,
            None => self.remotes.push(remote),
        }
    }

    /// Removes remote `name`; whether it existed.
    pub(crate) fn remove(&mut self, name: &str) -> bool {
        let before = self.remotes.len();
        self.remotes.retain(|remote| remote.name != name);
        self.remotes.len() != before
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Remote> {
        self.remotes.iter().find(|remote| remote.name == name)
    }

    /// Remote `name`, or why a watch of one of its sessions cannot go on.
    pub(crate) fn require(&self, name: &str) -> Result<&Remote, String> {
        self.get(name)
            .ok_or_else(|| format!("remote {name:?} was removed"))
    }
}

/// Where a call to a remote connects: the URL [`check_url`] passed, and for
/// plain `http` to a host name, the private addresses it resolved to then,
/// which the connection is pinned to, so a name that later resolves to a
/// public address never gets the secret in plaintext.
pub(crate) struct Endpoint {
    pub(crate) url: Url,
    /// Empty for `https` and for an address literal.
    pub(crate) pinned: Vec<SocketAddr>,
}

/// `url` as a remote's endpoint: `https`, or plain `http` to a host on a
/// private network (`private_network`), where the owner decided the network
/// keeps it private; and no userinfo, which would put a second secret in
/// the URL `list` shows. A host name for plain `http` is resolved here, and
/// every address it has must be private. Checked again at each call, so the
/// name is resolved again then.
pub(crate) async fn check_url(url: &str) -> anyhow::Result<Endpoint> {
    // Not echoed: a URL that fails to parse may still carry userinfo.
    let parsed =
        Url::parse(url).map_err(|error| anyhow::anyhow!("the remote URL is not valid: {error}"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        // Not echoed: the userinfo may be a secret.
        anyhow::bail!(
            "the remote URL carries a user name or password; pipe the secret on stdin instead"
        );
    }
    match parsed.scheme() {
        "https" => Ok(Endpoint {
            url: parsed,
            pinned: Vec::new(),
        }),
        "http" => {
            let (private, pinned) = match parsed.host() {
                Some(Host::Ipv4(address)) => (is_private(address.into()), Vec::new()),
                Some(Host::Ipv6(address)) => (is_private(address.into()), Vec::new()),
                Some(Host::Domain(domain)) => {
                    let port = parsed.port_or_known_default().unwrap_or(80);
                    let addresses: Vec<_> = tokio::net::lookup_host((domain, port))
                        .await
                        .map_err(|error| anyhow::anyhow!("could not resolve {domain}: {error}"))?
                        .collect();
                    let private = !addresses.is_empty()
                        && addresses.iter().all(|address| is_private(address.ip()));
                    (private, addresses)
                }
                None => anyhow::bail!("remote URL {url:?} names no host"),
            };
            if !private {
                anyhow::bail!(
                    "remote URL {url:?} must use https; plain http is allowed only to a loopback or private network address, such as 192.168.x.x or a NetBird or Tailscale peer's 100.x.y.z"
                );
            }
            Ok(Endpoint {
                url: parsed,
                pinned,
            })
        }
        scheme => anyhow::bail!("remote URL {url:?} must use https or http, not {scheme}"),
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
