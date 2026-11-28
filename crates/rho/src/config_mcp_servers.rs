//! The MCP servers `rho/mcp/config/*` edits entry by entry: `[mcp.servers]`
//! of `config.toml`. A server's `headers` and `env` maps hold secrets, so a
//! read names their entries without their values ([`mcp_server_entries`]),
//! and a put rebuilds them from slots ([`apply_mcp_changes`]): each value
//! is the one the file already holds or one a caller opened from a sealed
//! bundle. Plugin servers belong to their plugins and are not here.
//!
//! [`mcp_server`] and [`mcp_servers_revision`] see the whole entry, secret
//! values included, for the sync journal and for a revision a client cannot
//! learn the secrets from.

use std::collections::HashMap;

use hmac::{Hmac, KeyInit, Mac};
use rho_protocol::{
    ConfigRevision, McpSecretKind, McpSecretSlot, McpServerChange, McpServerEntry, SecretSource,
};
use serde_json::{Map, Value as Json};
use sha2::Sha256;

use super::{
    document::decode_table,
    entries::{self, ChangeError},
    Config, ConfigDocument, ConfigError, ConfigWarning,
};
use crate::tools::mcp::{config::McpServerConfig, validate_identity};

const SERVERS: [&str; 2] = ["mcp", "servers"];

/// Each kind of secret a server holds, with the map of the server that
/// holds it.
const SECRET_MAPS: [(McpSecretKind, &str); 2] = [
    (McpSecretKind::Header, "headers"),
    (McpSecretKind::Env, "env"),
];

/// Secret values a caller opened, by the server and slot each fills.
pub(crate) type SealedValues<'a> = HashMap<(String, McpSecretSlot), &'a str>;

/// The servers `doc` holds, by identity, as a read returns them: without
/// their secret values, each with the error that keeps the daemon from
/// loading it, if any.
pub(crate) fn mcp_server_entries(doc: &ConfigDocument) -> Vec<McpServerEntry> {
    entries::keyed_entries(doc, &SERVERS)
        .into_iter()
        .map(|(identity, value)| {
            let error = load_error(&identity, &value);
            let (value, secrets) = without_secrets(value);
            McpServerEntry::new(identity, value, secrets, error)
        })
        .collect()
}

/// Server `identity` as `doc` holds it, secret values included, as
/// canonical JSON text; `None` when `doc` holds no such server. Equal
/// entries give equal text, however the file orders or formats them.
pub(crate) fn mcp_server(doc: &ConfigDocument, identity: &str) -> Option<String> {
    doc.get_at(&entries::entry_path(&SERVERS, identity))
        .map(|item| canonical_text(&entries::to_json(item)))
}

/// The revision of `[mcp.servers]` as `doc` holds it: an HMAC-SHA-256 under
/// `key` of its canonical JSON, secret values included. Keyed, so a client
/// that knows the rest of an entry cannot test guesses of a short secret
/// against it.
pub(crate) fn mcp_servers_revision(doc: &ConfigDocument, key: &[u8]) -> ConfigRevision {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(&entries::canonical_json(&entries::keyed_entries(
        doc, &SERVERS,
    )));
    ConfigRevision::new(hex::encode(mac.finalize().into_bytes()))
}

/// The literal value `doc` holds for `slot` of server `identity`, if any.
pub(crate) fn mcp_secret(
    doc: &ConfigDocument,
    identity: &str,
    slot: &McpSecretSlot,
) -> Option<String> {
    let map = slot_map(slot.kind)?;
    doc.get_at(&[SERVERS[0], SERVERS[1], identity, map, &slot.name])
        .and_then(|item| item.as_str())
        .map(str::to_owned)
}

/// Applies `changes`, in order, to `[mcp.servers]` in `doc`. A put's secret
/// whose source is `keep` takes the value `doc` held before the changes;
/// one whose source is `sealed` takes its value from `sealed`. Every change
/// lands or none does: a put the loader refuses, a secret with no value, a
/// remove of a server the file lacks, or a file that would not load leaves
/// `doc` as it was. Returns the config `doc` now loads as.
pub(crate) fn apply_mcp_changes(
    doc: &mut ConfigDocument,
    changes: &[McpServerChange],
    sealed: &SealedValues,
) -> Result<(Config, Vec<ConfigWarning>), ChangeError> {
    let mut edited = doc.clone();
    let mut errors = Vec::new();
    let mut failed = Vec::new();
    for change in changes {
        // A later change to a server that failed would fail for that reason.
        if failed.contains(&change.identity()) {
            continue;
        }
        if let Err(message) = apply(doc, &mut edited, change, sealed) {
            errors.push(ConfigError::at(label(change.identity()), message));
            failed.push(change.identity());
        }
    }
    finish(
        doc,
        edited,
        errors,
        changes.iter().map(McpServerChange::identity),
    )
}

/// Writes back each server in `restores` from its canonical JSON text, as
/// [`mcp_server`] gives it, or removes it for `None`, then loads the file
/// as [`apply_mcp_changes`] does. Returns the config `doc` now loads as.
pub(crate) fn restore_mcp_servers(
    doc: &mut ConfigDocument,
    restores: &[(String, Option<String>)],
) -> Result<(Config, Vec<ConfigWarning>), ChangeError> {
    let mut edited = doc.clone();
    let mut errors = Vec::new();
    for (identity, entry) in restores {
        let item = entry
            .as_deref()
            .map(|entry| {
                serde_json::from_str::<Json>(entry)
                    .map_err(|error| error.to_string())
                    .and_then(|value| entries::to_item(&value))
            })
            .transpose();
        match item {
            Ok(item) => edited.set_item_at(&entries::entry_path(&SERVERS, identity), item),
            Err(message) => errors.push(ConfigError::at(label(identity), message)),
        }
    }
    finish(
        doc,
        edited,
        errors,
        restores.iter().map(|(identity, _)| identity.as_str()),
    )
}

/// Saves `edited` over `doc` once nothing was refused and it loads.
fn finish<'a>(
    doc: &mut ConfigDocument,
    edited: ConfigDocument,
    errors: Vec<ConfigError>,
    changed: impl Iterator<Item = &'a str>,
) -> Result<(Config, Vec<ConfigWarning>), ChangeError> {
    if !errors.is_empty() {
        return Err(ChangeError::Refused(errors));
    }
    let changed = changed
        .map(|identity| entries::entry_path(&SERVERS, identity))
        .collect::<Vec<_>>();
    let loaded = entries::load_edited(doc, &edited, &changed)?;
    *doc = edited;
    Ok(loaded)
}

fn apply(
    original: &ConfigDocument,
    edited: &mut ConfigDocument,
    change: &McpServerChange,
    sealed: &SealedValues,
) -> Result<(), String> {
    let path = entries::entry_path(&SERVERS, change.identity());
    match change {
        McpServerChange::Put {
            identity,
            value,
            secrets,
        } => {
            validate_identity(identity).map_err(|error| error.to_string())?;
            let mut value = value.clone();
            let fields = value.as_object_mut().ok_or("must be a table")?;
            if SECRET_MAPS.iter().any(|(_, map)| fields.contains_key(*map)) {
                return Err(
                    "must not hold `headers` or `env`: name each of their entries in `secrets`"
                        .into(),
                );
            }
            for secret in secrets {
                let name = &secret.slot.name;
                let map = slot_map(secret.slot.kind)
                    .ok_or("names a kind of secret this daemon does not know")?;
                let filled = match secret.source {
                    SecretSource::Keep => mcp_secret(original, identity, &secret.slot)
                        .ok_or_else(|| format!("keeps `{map}.{name}`, which it does not hold"))?,
                    SecretSource::Sealed => sealed
                        .get(&(identity.clone(), secret.slot.clone()))
                        .map(|value| (*value).to_owned())
                        .ok_or_else(|| format!("`{map}.{name}` is not in the sealed bundle"))?,
                };
                let entries = fields
                    .entry(map)
                    .or_insert_with(|| Json::Object(Map::new()))
                    .as_object_mut()
                    .expect("built as an object");
                if entries.insert(name.clone(), Json::String(filled)).is_some() {
                    return Err(format!("names `{map}.{name}` twice"));
                }
            }
            let item = entries::to_item(&value)?;
            decode_table::<McpServerConfig>(&item)?;
            edited.set_item_at(&path, Some(item));
        }
        McpServerChange::Remove { .. } => {
            edited
                .get_at(&path)
                .ok_or("the file holds no such server")?;
            edited.remove_at(&path);
        }
    }
    Ok(())
}

/// `value` without its `headers` and `env` maps, and the names those held.
fn without_secrets(mut value: Json) -> (Json, Vec<McpSecretSlot>) {
    let mut secrets = Vec::new();
    if let Json::Object(fields) = &mut value {
        for (kind, map) in SECRET_MAPS {
            if let Some(Json::Object(entries)) = fields.remove(map) {
                secrets.extend(entries.keys().map(|name| McpSecretSlot::new(kind, name)));
            }
        }
    }
    (value, secrets)
}

/// Why the daemon would not load server `identity` holding `value`.
fn load_error(identity: &str, value: &Json) -> Option<String> {
    if let Err(error) = validate_identity(identity) {
        return Some(error.to_string());
    }
    entries::to_item(value)
        .and_then(|item| decode_table::<McpServerConfig>(&item))
        .err()
}

/// The map of a server that holds secrets of `kind`.
fn slot_map(kind: McpSecretKind) -> Option<&'static str> {
    SECRET_MAPS
        .iter()
        .find(|(known, _)| *known == kind)
        .map(|(_, map)| *map)
}

fn canonical_text(value: &Json) -> String {
    String::from_utf8(entries::canonical_json(value)).expect("JSON is UTF-8")
}

/// How errors name server `identity`: its key in `config.toml`.
fn label(identity: &str) -> String {
    entries::dotted_key(&entries::entry_path(&SERVERS, identity))
}

#[cfg(test)]
#[path = "config_mcp_servers_tests.rs"]
mod tests;
