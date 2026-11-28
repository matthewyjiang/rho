//! `rho/sync/status` and `rho/sync/revert` over every kind of journaled
//! item: API keys, which [`keys`] reads and restores in the credential
//! store, and MCP servers, which [`mcp`] reads and restores in
//! `config.toml`. Both run under the journal lock, which every journaled
//! write takes too.

use rho_protocol::{
    RevertOutcome, RevertedItem, RpcError, SyncItemState, SyncItemStatus, SyncRevertParams,
    SyncRevertResponse, SyncStatusParams, SyncStatusResponse,
};

use super::{
    journal::{self, JournalItem, Place},
    keys, mcp, store_error, SecretSync,
};
use crate::{app::daemon_host::settings::ConfigStore, config::mcp_server};

/// An item a revert restores: its place in the revert's outcomes, what the
/// sync journaled of it, and whether the revert overwrites it even if it
/// changed since the sync.
pub(super) struct Pending {
    pub(super) index: usize,
    pub(super) journaled: JournalItem,
    pub(super) overwrite: bool,
}

/// What a revert did: the response, and whether it put back an API key,
/// which the sessions' providers then read again.
pub(in crate::app::daemon_host) struct Reverted {
    pub(in crate::app::daemon_host) response: SyncRevertResponse,
    pub(in crate::app::daemon_host) key_restored: bool,
}

impl SecretSync {
    /// Answers `rho/sync/status`.
    pub(in crate::app::daemon_host) async fn status(
        &self,
        params: SyncStatusParams,
        settings: &ConfigStore,
    ) -> Result<SyncStatusResponse, RpcError> {
        let SyncStatusParams { sync, .. } = params;
        let _held = self.journal.lock().await;
        // Read whether or not a server is journaled, but only a server's
        // state fails on a file that does not parse.
        let doc = settings.agent_document();
        self.offload(move |stores, now| {
            let journal = keys::journal_of(stores, &sync, now)?;
            let items = journal
                .items
                .iter()
                .map(|journaled| {
                    let current = match &journaled.place {
                        Place::ApiKey { account, .. } => {
                            keys::current(stores.keys, account).map_err(store_error)?
                        }
                        Place::McpServer { identity } => {
                            mcp_server(doc.as_ref().map_err(Clone::clone)?, identity)
                                .map(Into::into)
                        }
                    };
                    let state = if journaled.unchanged(current.as_deref().map(String::as_str)) {
                        SyncItemState::Unchanged
                    } else {
                        SyncItemState::Changed
                    };
                    Ok(SyncItemStatus::new(journaled.place.item(), state))
                })
                .collect::<Result<_, RpcError>>()?;
            Ok(SyncStatusResponse::new(journal.expires_at, items))
        })
        .await
    }

    /// Answers `rho/sync/revert`: restores each item that still holds what
    /// the sync wrote, or that `overwrite` names, and forgets the ones
    /// restored.
    pub(in crate::app::daemon_host) async fn revert(
        &self,
        params: SyncRevertParams,
        settings: &ConfigStore,
    ) -> Result<Reverted, RpcError> {
        let SyncRevertParams {
            sync,
            items,
            overwrite,
            ..
        } = params;
        let _held = self.journal.lock().await;
        let mut journal = self
            .offload(move |stores, now| keys::journal_of(stores, &sync, now))
            .await?;
        let mut outcomes = Vec::with_capacity(items.len());
        let mut key_reverts = Vec::new();
        let mut server_reverts = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let Some(journaled) = journal.find(item) else {
                outcomes.push((
                    index,
                    RevertOutcome::Failed {
                        message: "this sync did not change it, or it is already reverted".into(),
                    },
                ));
                continue;
            };
            let pending = Pending {
                index,
                journaled: journaled.clone(),
                overwrite: overwrite.contains(item),
            };
            match &journaled.place {
                Place::ApiKey { account, .. } => key_reverts.push((account.clone(), pending)),
                Place::McpServer { identity } => server_reverts.push((identity.clone(), pending)),
            }
        }
        let restored_keys = self
            .offload(move |stores, _| {
                Ok(key_reverts
                    .into_iter()
                    .map(|(account, pending)| {
                        let outcome = keys::restore(
                            stores.keys,
                            &account,
                            &pending.journaled,
                            pending.overwrite,
                        )
                        .unwrap_or_else(|error| RevertOutcome::Failed {
                            message: format!("could not restore it: {error}"),
                        });
                        (pending.index, outcome)
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        let key_restored = restored_keys
            .iter()
            .any(|(_, outcome)| *outcome == RevertOutcome::Restored);
        outcomes.extend(restored_keys);
        outcomes.extend(mcp::restore_servers(settings, server_reverts).await);
        outcomes.sort_unstable_by_key(|(index, _)| *index);
        let mut restored_any = false;
        for (index, outcome) in &outcomes {
            if *outcome == RevertOutcome::Restored {
                journal.remove(&items[*index]);
                restored_any = true;
            }
        }
        if restored_any {
            let saved = self
                .offload(move |stores, _| {
                    journal::save(stores.journal, &journal).map_err(store_error)
                })
                .await;
            if let Err(error) = saved {
                // What came back stays journaled and reads as changed, and
                // restoring it again with `overwrite` writes the same value.
                tracing::warn!(error = %error.message, "could not update the sync journal after a revert");
            }
        }
        debug_assert_eq!(outcomes.len(), items.len(), "one outcome per item");
        let reverted = items
            .into_iter()
            .zip(outcomes)
            .map(|(item, (_, outcome))| RevertedItem::new(item, outcome))
            .collect();
        Ok(Reverted {
            response: SyncRevertResponse::new(reverted),
            key_restored,
        })
    }
}

#[cfg(test)]
#[path = "revert_tests.rs"]
mod tests;
