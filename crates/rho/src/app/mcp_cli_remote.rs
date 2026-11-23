//! `rho mcp remote`: the other machines' `rho mcp serve --http` that this
//! machine's `rho mcp serve` drives as a hub (`app::mcp_server::remotes`).
//! The secret, the token a remote issued this machine, is read from stdin
//! only, so `ssh` can pipe it straight from the remote's `rho mcp token
//! rotate` without it reaching a terminal, argv, or shell history. `add`
//! checks the remote answers before it saves it.

use serde::Serialize;
use serde_json::{Map, Value};

use super::{serve::open_credential_store, webhook::read_secret};
use crate::{
    app::{
        mcp_server::{
            remotes::{self, check_name, check_url, Remote, RemoteError, RemoteSet},
            token_store,
        },
        session_control::Action,
    },
    caller_context::CallerContext,
    cli::{Cli, McpRemoteCommand},
    credential_store::UntrackedCredentialStore,
};

pub(in crate::app) async fn remote(command: &McpRemoteCommand, cli: &Cli) -> anyhow::Result<()> {
    open_credential_store(cli)?;
    match command {
        McpRemoteCommand::Add { name, url } => {
            // Before the secret is read, so a typo fails before anything else.
            check_name(name)?;
            let url = check_url(url).await?.url;
            let secret = read_secret(&format!(
                "pipe the secret of the token {name} issued this machine on stdin, as `ssh {name} rho mcp token rotate hub --scope write --workspace DIR | rho mcp remote add {name} --url {url}`"
            ))?;
            if secret.trim().is_empty() {
                anyhow::bail!(
                    "stdin was empty, so remote {name:?} was not saved: the command that should print the secret failed. Over SSH, `rho` is often not on the PATH of a non-interactive shell; give its full path, as `ssh {name} '~/.local/bin/rho mcp token rotate hub ...'`"
                );
            }
            let remote = Remote::new(name, url, &secret)?;
            probe(&remote).await?;
            let url = remote.url.clone();
            token_store::update(|remotes: &mut RemoteSet| {
                remotes.upsert(remote);
                Ok(())
            })?;
            println!(
                "saved remote {name:?} at {url}; it answered. Every token of this machine's `rho mcp serve` reaches it, as far as the token {name} issued allows"
            );
        }
        McpRemoteCommand::List { json } => {
            let remotes = RemoteSet::load(&UntrackedCredentialStore)?;
            let listing: Vec<Entry<'_>> = remotes
                .remotes
                .iter()
                .map(|remote| Entry {
                    name: &remote.name,
                    url: &remote.url,
                })
                .collect();
            if *json {
                println!("{}", serde_json::to_string_pretty(&listing)?);
            } else if listing.is_empty() {
                println!("no MCP server remotes; add one with `rho mcp remote add`");
            } else {
                for entry in &listing {
                    println!("{}  {}", entry.name, entry.url);
                }
            }
        }
        McpRemoteCommand::Remove { name } => {
            token_store::update(|remotes: &mut RemoteSet| {
                if !remotes.remove(name) {
                    anyhow::bail!("no MCP server remote named {name:?}");
                }
                Ok(())
            })?;
            println!("removed remote {name:?}");
        }
    }
    Ok(())
}

/// Lists one session on `remote`, which proves it answers and takes the
/// secret; nothing is saved unless it does. The secret piped in is lost
/// then, so the error says how to find the fault and that running the
/// line again is safe: the next rotate replaces the lost secret.
async fn probe(remote: &Remote) -> anyhow::Result<()> {
    let arguments = Map::from_iter([("limit".to_owned(), Value::from(1))]);
    remotes::call(
        remote,
        Action::List,
        arguments,
        &CallerContext::ROOT,
        remotes::Asked::ByCall,
    )
    .await
    .map(drop)
    .map_err(|error| {
        let name = &remote.name;
        let url = &remote.url;
        let causes = match error {
            RemoteError::Unreachable { .. } => format!(
                "\nlikely causes: a firewall or network policy (such as a NetBird or Tailscale ACL) blocks this machine from the port; the address or port is wrong; or `rho mcp serve --http` is not running there, or listens on another address. `curl -sS -o /dev/null -w '%{{http_code}}\\n' {url}` from this machine prints 401 once it is reachable"
            ),
            RemoteError::Refused { .. } | RemoteError::Failed { .. } => String::new(),
        };
        anyhow::anyhow!(
            "{error}; remote {name:?} was not saved{causes}\nrunning the same line again is safe: rotating the token again replaces the secret that was not saved"
        )
    })
}

/// One remote as `list` shows it.
#[derive(Serialize)]
struct Entry<'a> {
    name: &'a str,
    url: &'a str,
}
