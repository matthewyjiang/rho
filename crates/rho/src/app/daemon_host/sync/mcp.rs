//! `rho/mcp/config/set`, and the MCP servers a revert restores. A set may
//! fill secret values from a bundle sealed to this daemon and journal each
//! server's whole entry under a sync, so the target, which alone holds the
//! secret values, can put it back.
//!
//! A journaled set takes the journal lock before `config.toml`'s, as every
//! journaled write takes them, and saves the journal before the file
//! ([`Draft`]).

use std::{collections::HashSet, time::Instant};

use rho_protocol::{
    ConfigRevision, McpConfigSetParams, McpConfigSetResponse, McpServerChange, RevertOutcome,
    RpcError, SealedSecrets, SecretItem, SecretSource,
};
use zeroize::Zeroizing;

use super::{
    invalid,
    journal::{Draft, Place},
    open_values,
    revert::Pending,
    store_error,
    values::{mcp_item, mcp_slot},
    SecretSync,
};
use crate::{
    app::daemon_host::settings::{AppliedMcpEdit, ConfigStore},
    config::{apply_mcp_changes, mcp_server, restore_mcp_servers, ConfigDocument, SealedValues},
};

/// Each server a set changes, by identity, with its entry from before as
/// [`mcp_server`] gives it.
type Before = Vec<(String, Option<String>)>;

impl SecretSync {
    /// Answers `rho/mcp/config/set`: applies the changes while the servers
    /// are still at the params' revision, filling sealed secrets from
    /// `sealed`, and journals each server's old entry under `sync`, if any.
    pub(in crate::app::daemon_host) async fn set_mcp(
        &self,
        params: McpConfigSetParams,
        settings: &ConfigStore,
    ) -> Result<McpConfigSetResponse, RpcError> {
        let McpConfigSetParams {
            revision,
            changes,
            sealed,
            sync,
            ..
        } = params;
        refuse_unasked(&changes, sealed.as_ref())?;
        let Some(sync) = sync else {
            let (applied, _) = self
                .apply_mcp(settings, &revision, &changes, sealed.as_ref())
                .await?;
            return applied.save().map(McpConfigSetResponse::new);
        };
        let _held = self.journal.lock().await;
        let mut draft = self
            .offload(move |stores, now| {
                Draft::begin(stores.journal, &sync, now).map_err(store_error)
            })
            .await?;
        let (applied, before) = self
            .apply_mcp(settings, &revision, &changes, sealed.as_ref())
            .await?;
        for (identity, previous) in before {
            let written = mcp_server(applied.doc(), &identity);
            draft
                .journal
                .record(Place::McpServer { identity }, previous, written.as_deref());
        }
        let draft = self
            .offload(move |stores, _| {
                draft.save(stores.journal).map_err(store_error)?;
                Ok(draft)
            })
            .await?;
        match applied.save() {
            Ok(revision) => Ok(McpConfigSetResponse::new(revision)),
            Err(error) => {
                let abandoned = self
                    .offload(move |stores, _| draft.abandon(stores.journal).map_err(store_error))
                    .await;
                if let Err(abandoned) = abandoned {
                    // Its servers read as changed, so a revert leaves them
                    // unless told to overwrite.
                    tracing::warn!(error = %abandoned.message, "could not put back the sync journal after a failed MCP write");
                }
                Err(error)
            }
        }
    }

    /// Opens the MCP servers at `revision`, then `sealed`, and applies
    /// `changes`. Opening waits for the revision check, so a stale one
    /// leaves the recipient usable.
    async fn apply_mcp<'a>(
        &self,
        settings: &'a ConfigStore,
        revision: &ConfigRevision,
        changes: &[McpServerChange],
        sealed: Option<&SealedSecrets>,
    ) -> Result<(AppliedMcpEdit<'a>, Before), RpcError> {
        let edit = settings.edit_mcp_at(revision).await?;
        let values = match sealed {
            Some(sealed) => open_values(&self.recipients, Instant::now(), sealed)?,
            None => Zeroizing::new(Vec::new()),
        };
        let filled = sealed
            .iter()
            .flat_map(|sealed| &sealed.items)
            .zip(values.iter())
            .filter_map(|(item, value)| {
                mcp_slot(item).map(|(server, slot)| ((server.to_owned(), slot), value.as_str()))
            })
            .collect::<SealedValues>();
        let mut identities = changes
            .iter()
            .map(McpServerChange::identity)
            .collect::<Vec<_>>();
        identities.sort_unstable();
        identities.dedup();
        let before = identities
            .into_iter()
            .map(|identity| (identity.to_owned(), mcp_server(edit.doc(), identity)))
            .collect();
        let applied = edit.apply(|doc| apply_mcp_changes(doc, changes, &filled))?;
        Ok((applied, before))
    }
}

/// Restores each of `servers`, by identity, that still holds what the sync
/// wrote, or that its revert overwrites. They are saved together, so a file
/// that would not load with them fails each; when none is restored the file
/// is left as it is.
pub(super) async fn restore_servers(
    settings: &ConfigStore,
    servers: Vec<(String, Pending)>,
) -> Vec<(usize, RevertOutcome)> {
    if servers.is_empty() {
        return Vec::new();
    }
    let fail = |message: &str, outcomes: Vec<(usize, RevertOutcome)>| {
        outcomes
            .into_iter()
            .map(|(index, outcome)| match outcome {
                RevertOutcome::Restored => (
                    index,
                    RevertOutcome::Failed {
                        message: format!("could not restore it: {message}"),
                    },
                ),
                outcome => (index, outcome),
            })
            .collect()
    };
    let edit = match settings.edit_mcp().await {
        Ok(edit) => edit,
        Err(error) => {
            let all = servers
                .iter()
                .map(|(_, pending)| (pending.index, RevertOutcome::Restored))
                .collect();
            return fail(&error.message, all);
        }
    };
    let (restores, outcomes) = plan(edit.doc(), servers);
    if restores.is_empty() {
        return outcomes;
    }
    match edit
        .apply(|doc| restore_mcp_servers(doc, &restores))
        .and_then(AppliedMcpEdit::save)
    {
        Ok(_) => outcomes,
        Err(error) => fail(&error.message, outcomes),
    }
}

/// Which of `servers` a revert restores from `doc`: the entries to write
/// back, and each server's outcome once they are written.
fn plan(
    doc: &ConfigDocument,
    servers: Vec<(String, Pending)>,
) -> (Before, Vec<(usize, RevertOutcome)>) {
    let mut restores = Vec::new();
    let mut outcomes = Vec::with_capacity(servers.len());
    for (identity, pending) in servers {
        let current = mcp_server(doc, &identity);
        if pending.journaled.unchanged(current.as_deref()) || pending.overwrite {
            restores.push((identity, pending.journaled.previous.clone()));
            outcomes.push((pending.index, RevertOutcome::Restored));
        } else {
            outcomes.push((pending.index, RevertOutcome::Changed));
        }
    }
    (restores, outcomes)
}

/// Refuses, before opening, a bundle whose items are not exactly the
/// secrets the puts take from it, so its recipient stays usable.
fn refuse_unasked(
    changes: &[McpServerChange],
    sealed: Option<&SealedSecrets>,
) -> Result<(), RpcError> {
    let asked = changes
        .iter()
        .flat_map(|change| match change {
            McpServerChange::Put {
                identity, secrets, ..
            } => secrets
                .iter()
                .filter(|secret| secret.source == SecretSource::Sealed)
                .filter_map(|secret| mcp_item(identity, &secret.slot))
                .collect(),
            McpServerChange::Remove { .. } => Vec::new(),
        })
        .collect::<HashSet<SecretItem>>();
    let sent = sealed
        .iter()
        .flat_map(|sealed| sealed.items.iter().cloned())
        .collect::<HashSet<_>>();
    if asked != sent {
        return Err(invalid(
            "`sealed` must hold exactly the secrets the puts take from it",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
