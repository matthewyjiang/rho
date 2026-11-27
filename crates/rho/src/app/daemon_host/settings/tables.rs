//! `rho/config/table/*`: the structured tables of `config.toml` a client
//! copies between daemons row by row, such as provider profiles, read and
//! written entry by entry (`crate::config::read_table`).
//!
//! A table's revision is its own, so a table write never makes a pending
//! `rho/config/set` stale. A write that moves a setting's value, such as an
//! alias an `@alias` model names, publishes the agent namespace's new state
//! as `rho/config/set` does, and the config reaches turns and sessions the
//! same way.

use rho_protocol::{
    ConfigTable, ConfigTableGetParams, ConfigTableGetResponse, ConfigTableSetParams,
    ConfigTableSetResponse, ErrorCode, RpcError,
};

use super::{change_error, file, ConfigStore};
use crate::config::{apply_table_changes, read_table, table_revision, EditableTable};

impl ConfigStore {
    /// Answers `rho/config/table/get`: the table as `config.toml` holds it,
    /// whether or not the file loads.
    pub(in crate::app::daemon_host) async fn get_table(
        &self,
        params: ConfigTableGetParams,
    ) -> Result<ConfigTableGetResponse, RpcError> {
        let table = editable(params.table)?;
        let (revision, entries) = read_table(&file::read_document(&self.agent_path)?, table);
        Ok(ConfigTableGetResponse::new(revision, entries))
    }

    /// Answers `rho/config/table/set`: applies every change, or none, while
    /// the table is still at the params' revision and the file still loads
    /// with them.
    pub(in crate::app::daemon_host) async fn set_table(
        &self,
        params: ConfigTableSetParams,
    ) -> Result<ConfigTableSetResponse, RpcError> {
        let table = editable(params.table)?;
        let mut file = self.agent.lock().await;
        // Locked before the revision check, so the table the client read is
        // the one changed.
        let mut edit = file::Edit::open(&self.agent_path)?;
        if table_revision(&edit.doc, table) != params.revision {
            return Err(RpcError::new(
                ErrorCode::ConfigConflict,
                "the table changed since that revision; read it again",
            ));
        }
        let loaded =
            apply_table_changes(&mut edit.doc, table, &params.changes).map_err(change_error)?;
        let (doc, _) = self.save_agent(&mut file, edit, loaded)?;
        Ok(ConfigTableSetResponse::new(table_revision(&doc, table)))
    }
}

fn editable(table: ConfigTable) -> Result<EditableTable, RpcError> {
    EditableTable::from_wire(table)
        .ok_or_else(|| RpcError::new(ErrorCode::InvalidParams, "unknown config table"))
}
