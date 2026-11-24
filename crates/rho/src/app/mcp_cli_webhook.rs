//! `rho mcp webhook`: the targets `rho mcp serve --http` POSTs watched
//! sessions' news to. The secret is read from stdin only, never from argv,
//! where shell history and process lists would keep it, and never printed.
//! `list` shows each target's last delivery and the watches stalled on it,
//! and each token's watches against the server's `--max-watches`, from the
//! server's watch store.

use std::{collections::BTreeMap, io::Read};

use serde::Serialize;

use super::serve::open_credential_store;
use crate::{
    app::mcp_server::{
        token_store,
        watch::{self, DeliveryRecord, DeliveryResult, WatchFile, WatchStore},
        webhooks::{self, WebhookSet},
    },
    cli::{Cli, McpWebhookCommand},
    credential_store::UntrackedCredentialStore,
};

pub(in crate::app) fn webhook(command: &McpWebhookCommand, cli: &Cli) -> anyhow::Result<()> {
    open_credential_store(cli)?;
    match command {
        McpWebhookCommand::Add { name, url, token } => {
            // Before the secret is read, so a typo does not cost a paste.
            webhooks::validate_url(url)?;
            let secret = read_secret(
                "pipe the webhook's secret on stdin, as `printf %s \"$KEY\" | rho mcp webhook add ...`, so it stays out of your shell history",
            )?;
            token_store::update(|webhooks: &mut WebhookSet| {
                webhooks.upsert(name, url, &secret, token)
            })?;
            println!(
                "saved webhook {name:?} for {}",
                token
                    .iter()
                    .map(|token| format!("token {token:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        McpWebhookCommand::List { json } => {
            let webhooks = WebhookSet::load(&UntrackedCredentialStore)?;
            let watches = WatchStore::new(crate::paths::rho_dir()?.join(watch::DIR)).load()?;
            let listing = list(&webhooks, &watches);
            if *json {
                println!("{}", serde_json::to_string_pretty(&listing)?);
            } else if listing.webhooks.is_empty() && listing.watches.is_empty() {
                println!("no MCP server webhooks; add one with `rho mcp webhook add`");
            } else {
                for entry in &listing.webhooks {
                    print_entry(entry);
                }
                for count in &listing.watches {
                    print_count(count, listing.max_watches);
                }
            }
        }
        McpWebhookCommand::Remove { name } => {
            token_store::update(|webhooks: &mut WebhookSet| {
                if !webhooks.remove(name) {
                    anyhow::bail!("no MCP server webhook named {name:?}");
                }
                Ok(())
            })?;
            println!("removed webhook {name:?}");
        }
    }
    Ok(())
}

/// The secret piped on stdin: one line, its line ending dropped. On a
/// terminal, or when stdin holds nothing but whitespace, as when the
/// command piping it failed, fails with `hint`, which says how to pipe it.
pub(super) fn read_secret(hint: &str) -> anyhow::Result<String> {
    if crate::stdio::stdin_is_terminal() {
        anyhow::bail!("{hint}");
    }
    let mut secret = String::new();
    std::io::stdin().read_to_string(&mut secret)?;
    if secret.trim().is_empty() {
        anyhow::bail!("stdin was empty, so nothing was saved: {hint}");
    }
    Ok(secret.trim_end_matches(['\r', '\n']).to_owned())
}

/// What `list` shows.
#[derive(Debug, Serialize)]
struct Listing<'a> {
    webhooks: Vec<Entry<'a>>,
    /// Each token's watches, by token, as `--max-watches` counts them.
    watches: Vec<TokenWatches<'a>>,
    /// The `--max-watches` of the server that last saved the watches.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_watches: Option<u32>,
}

/// A token's watches: those still following their sessions, and those
/// stalled. Both count toward its budget.
#[derive(Debug, Default, Serialize)]
struct TokenWatches<'a> {
    token: &'a str,
    active: usize,
    stalled: usize,
}

/// One webhook as `list` shows it.
#[derive(Debug, Serialize)]
struct Entry<'a> {
    name: &'a str,
    url: &'a str,
    tokens: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    last_delivery: Option<&'a DeliveryRecord>,
    /// Watches stopped short on this webhook.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stalled: Vec<Stalled<'a>>,
}

#[derive(Debug, Serialize)]
struct Stalled<'a> {
    session: &'a str,
    token: &'a str,
    error: &'a str,
}

fn list<'a>(webhooks: &'a WebhookSet, watches: &'a WatchFile) -> Listing<'a> {
    let mut counts: BTreeMap<&str, TokenWatches<'a>> = BTreeMap::new();
    for watch in &watches.watches {
        let count = counts.entry(&watch.token).or_insert_with(|| TokenWatches {
            token: &watch.token,
            ..TokenWatches::default()
        });
        match watch.error {
            Some(_) => count.stalled += 1,
            None => count.active += 1,
        }
    }
    Listing {
        webhooks: entries(webhooks, watches),
        watches: counts.into_values().collect(),
        max_watches: watches.max_watches,
    }
}

fn entries<'a>(webhooks: &'a WebhookSet, watches: &'a WatchFile) -> Vec<Entry<'a>> {
    webhooks
        .targets
        .iter()
        .map(|target| Entry {
            name: &target.name,
            url: &target.url,
            tokens: &target.tokens,
            last_delivery: watches.deliveries.get(&target.name),
            stalled: watches
                .watches
                .iter()
                .filter(|watch| watch.webhook == target.name)
                .filter_map(|watch| {
                    Some(Stalled {
                        session: watch.session.as_str(),
                        token: &watch.token,
                        error: watch.error.as_deref()?,
                    })
                })
                .collect(),
        })
        .collect()
}

fn print_entry(entry: &Entry<'_>) {
    println!(
        "{}  {}  tokens: {}",
        entry.name,
        entry.url,
        entry.tokens.join(", ")
    );
    if let Some(last) = entry.last_delivery {
        let when = humantime::format_rfc3339_seconds(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(last.at),
        );
        match &last.result {
            DeliveryResult::Delivered(status) => println!(
                "  last delivery {when}: session {}, answer {status}",
                last.session
            ),
            DeliveryResult::Failed(error) => println!(
                "  last delivery {when}: session {}, failed: {error}",
                last.session
            ),
        }
    }
    for stalled in &entry.stalled {
        println!(
            "  stalled: session {} (token {:?}): {}",
            stalled.session, stalled.token, stalled.error
        );
    }
}

fn print_count(count: &TokenWatches<'_>, max_watches: Option<u32>) {
    let total = count.active + count.stalled;
    let of = max_watches.map_or_else(String::new, |limit| format!(" of --max-watches {limit}"));
    println!(
        "token {:?}: {total} watches{of} ({} active, {} stalled)",
        count.token, count.active, count.stalled
    );
}
