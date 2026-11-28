//! What each kind of sealed item holds on this daemon: an API key in the
//! credential store, or a header or env value of an MCP server in
//! `config.toml`. Export reads them to seal; compare reads them to match.

use rho_protocol::{
    CompareState, McpSecretKind, McpSecretSlot, RpcError, SecretComparison, SecretItem,
};
use rho_providers::credentials::CredentialStore;
use zeroize::Zeroizing;

use super::{digest, invalid, refuse_duplicates};
use crate::{
    app::credential_writes::{self, CredentialWriteError},
    config::{mcp_secret, ConfigDocument},
};

/// What [`saved_values`] found: the items that have a saved value, with
/// those values in the same order, and the items that have none.
pub(super) struct Saved {
    pub(super) items: Vec<SecretItem>,
    pub(super) values: Zeroizing<Vec<String>>,
    pub(super) missing: Vec<SecretItem>,
}

/// Reads each item's saved value from `store` or `doc`, refusing an item
/// this daemon cannot export. A blank saved key reads as none; an MCP
/// server's value is taken as the file holds it.
pub(super) fn saved_values(
    store: &dyn CredentialStore,
    doc: &ConfigDocument,
    items: Vec<SecretItem>,
) -> Result<Saved, RpcError> {
    refuse_duplicates(&items)?;
    let mut saved = Saved {
        items: Vec::new(),
        values: Zeroizing::new(Vec::new()),
        missing: Vec::new(),
    };
    for item in items {
        let value = match mcp_slot(&item) {
            Some((server, slot)) => mcp_secret(doc, server, &slot),
            None => credential_writes::saved_api_key(api_key_auth(&item)?, store)?,
        };
        match value {
            Some(value) => {
                saved.items.push(item);
                saved.values.push(value);
            }
            None => saved.missing.push(item),
        }
    }
    Ok(saved)
}

/// How each of `values`, one per item of `items`, compares with the value
/// `store` or `doc` holds for it.
pub(super) fn compare_values(
    store: &dyn CredentialStore,
    doc: &ConfigDocument,
    items: Vec<SecretItem>,
    values: &[String],
) -> Result<Vec<SecretComparison>, RpcError> {
    items
        .into_iter()
        .zip(values)
        .map(|(item, value)| {
            let held = match mcp_slot(&item) {
                Some((server, slot)) => mcp_secret(doc, server, &slot),
                // A profile the target does not know yet, such as a custom
                // provider only the source defines, holds nothing here.
                None => match credential_writes::saved_api_key(api_key_auth(&item)?, store) {
                    Err(CredentialWriteError::UnknownAuth { .. }) => None,
                    held => held?,
                },
            }
            .map(Zeroizing::new);
            // Comparing digests keeps the time it takes from telling how
            // much of a guessed value is right.
            let state = match held {
                None => CompareState::Absent,
                Some(held) if digest(&held) == digest(value) => CompareState::Same,
                Some(_) => CompareState::Differs,
            };
            Ok(SecretComparison::new(item, state))
        })
        .collect()
}

/// The auth profile whose saved API key `item` names.
pub(super) fn api_key_auth(item: &SecretItem) -> Result<&str, RpcError> {
    match item {
        SecretItem::ApiKey { auth, .. } => Ok(auth),
        // An MCP server's value, `Unknown`, or a kind a newer protocol adds.
        _ => Err(invalid("that item is not a saved API key")),
    }
}

/// The sealed item that carries `slot` of MCP server `identity`; `None` for
/// a kind of secret this daemon does not know.
pub(super) fn mcp_item(identity: &str, slot: &McpSecretSlot) -> Option<SecretItem> {
    match slot.kind {
        McpSecretKind::Header => Some(SecretItem::mcp_header(identity, &slot.name)),
        McpSecretKind::Env => Some(SecretItem::mcp_env(identity, &slot.name)),
        // `Unknown`, or a kind a newer protocol adds.
        _ => None,
    }
}

/// The MCP server and slot `item` carries; `None` for an item that is not
/// an MCP server's secret.
pub(super) fn mcp_slot(item: &SecretItem) -> Option<(&str, McpSecretSlot)> {
    match item {
        SecretItem::McpHeader { server, name, .. } => {
            Some((server, McpSecretSlot::new(McpSecretKind::Header, name)))
        }
        SecretItem::McpEnv { server, name, .. } => {
            Some((server, McpSecretSlot::new(McpSecretKind::Env, name)))
        }
        _ => None,
    }
}
