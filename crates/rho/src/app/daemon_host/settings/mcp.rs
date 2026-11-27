//! `rho/mcp/config/*`'s side of the settings store: `[mcp.servers]` read
//! without its secret values, and a locked edit of it that the sync module
//! drives, so it can journal what a write replaces between checking the
//! revision and saving (`crate::config::apply_mcp_changes`).
//!
//! The revision is keyed with a secret this daemon process draws once, so
//! it tells a client nothing about the secret values it covers; a restart
//! makes every earlier revision stale.

use std::sync::OnceLock;

use rho_protocol::{ConfigRevision, ErrorCode, McpConfigGetResponse, RpcError};
use tokio::sync::MutexGuard;

use super::{change_error, file, AgentFile, ConfigStore};
use crate::config::{
    mcp_server_entries, mcp_servers_revision, ChangeError, Config, ConfigDocument, ConfigWarning,
};

impl ConfigStore {
    /// `config.toml` as it is now, whether or not it loads.
    pub(in crate::app::daemon_host) fn agent_document(&self) -> Result<ConfigDocument, RpcError> {
        file::read_document(&self.agent_path)
    }

    /// Answers `rho/mcp/config/get`.
    pub(in crate::app::daemon_host) fn mcp_config(&self) -> Result<McpConfigGetResponse, RpcError> {
        let doc = self.agent_document()?;
        Ok(McpConfigGetResponse::new(
            mcp_revision(&doc),
            mcp_server_entries(&doc),
        ))
    }

    /// Opens `config.toml` to edit its MCP servers while they are still at
    /// `revision`, refusing with `ConfigConflict` otherwise. The file stays
    /// locked against every other writer until the edit is saved or dropped.
    pub(in crate::app::daemon_host) async fn edit_mcp_at(
        &self,
        revision: &ConfigRevision,
    ) -> Result<McpEdit<'_>, RpcError> {
        // Locked before the revision check, so the servers the client read
        // are the ones changed.
        let edit = self.edit_mcp().await?;
        if *revision != mcp_revision(edit.doc()) {
            return Err(RpcError::new(
                ErrorCode::ConfigConflict,
                "the MCP servers changed since that revision; read them again",
            ));
        }
        Ok(edit)
    }

    /// Opens `config.toml` to edit its MCP servers as they are now, as
    /// [`Self::edit_mcp_at`] does without the revision check.
    pub(in crate::app::daemon_host) async fn edit_mcp(&self) -> Result<McpEdit<'_>, RpcError> {
        let file = self.agent.lock().await;
        let edit = file::Edit::open(&self.agent_path)?;
        Ok(McpEdit {
            store: self,
            file,
            edit,
        })
    }
}

/// An open edit of `config.toml`'s MCP servers ([`ConfigStore::edit_mcp`]).
pub(in crate::app::daemon_host) struct McpEdit<'a> {
    store: &'a ConfigStore,
    file: MutexGuard<'a, AgentFile>,
    edit: file::Edit,
}

impl<'a> McpEdit<'a> {
    /// The file as it is now.
    pub(in crate::app::daemon_host) fn doc(&self) -> &ConfigDocument {
        &self.edit.doc
    }

    /// Applies `change`, which edits the document and returns what it now
    /// loads as, or leaves it as it was and says why not.
    pub(in crate::app::daemon_host) fn apply(
        mut self,
        change: impl FnOnce(&mut ConfigDocument) -> Result<(Config, Vec<ConfigWarning>), ChangeError>,
    ) -> Result<AppliedMcpEdit<'a>, RpcError> {
        let loaded = change(&mut self.edit.doc).map_err(change_error)?;
        Ok(AppliedMcpEdit { open: self, loaded })
    }
}

/// An edit of the MCP servers with its change applied, ready to save.
pub(in crate::app::daemon_host) struct AppliedMcpEdit<'a> {
    open: McpEdit<'a>,
    /// What the changed file loads as.
    loaded: (Config, Vec<ConfigWarning>),
}

impl AppliedMcpEdit<'_> {
    /// The file with the change applied.
    pub(in crate::app::daemon_host) fn doc(&self) -> &ConfigDocument {
        self.open.doc()
    }

    /// Saves the change, publishing the config it loads as, and returns
    /// the servers' new revision.
    pub(in crate::app::daemon_host) fn save(self) -> Result<ConfigRevision, RpcError> {
        let McpEdit {
            store,
            mut file,
            edit,
        } = self.open;
        let (doc, _) = store.save_agent(&mut file, edit, self.loaded)?;
        Ok(mcp_revision(&doc))
    }
}

/// The revision of the MCP servers `doc` holds, under this process's key.
fn mcp_revision(doc: &ConfigDocument) -> ConfigRevision {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = KEY.get_or_init(|| {
        use rand::{rand_core::UnwrapErr, rngs::SysRng, Rng};
        let mut key = [0_u8; 32];
        UnwrapErr(SysRng).fill_bytes(&mut key);
        key
    });
    mcp_servers_revision(doc, key)
}
