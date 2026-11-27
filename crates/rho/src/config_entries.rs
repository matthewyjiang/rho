//! Entries of `config.toml`'s structured tables as JSON, for the protocol
//! methods that read and write a table entry by entry, such as
//! `rho/config/table/*`. The feature that owns a table says where its
//! entries sit and how a put is checked. This module holds the parts every
//! such table shares:
//!
//! - an entry's TOML as JSON, and back;
//! - the entries of a table keyed by name, at a key path whose segments may
//!   hold dots;
//! - the canonical JSON of what a read returned, and a revision over it;
//! - loading the edited file whole before anything is saved.
//!
//! JSON carries every field the file holds, so an entry copied between
//! daemons keeps the fields the protocol never names. The daemon receiving it
//! decodes it with its loader's own serde types, which refuse a field they do
//! not know.

use std::collections::BTreeMap;

use rho_protocol::ConfigRevision;
use serde::Serialize;
use serde_json::{Map, Value as Json};
use sha2::{Digest, Sha256};
use toml_edit::{InlineTable, Item, TableLike, Value};

use super::{document::standard_table, load, Config, ConfigDocument, ConfigError, ConfigWarning};

/// `item` as JSON: a table is an object and an array of tables an array of
/// objects. A datetime becomes its TOML text, and a float JSON cannot hold
/// (`nan`, `inf`) becomes `null`, which [`to_item`] refuses.
pub(crate) fn to_json(item: &Item) -> Json {
    match item {
        Item::None => Json::Null,
        Item::Value(value) => value_json(value),
        Item::Table(table) => table_json(table),
        Item::ArrayOfTables(tables) => {
            Json::Array(tables.iter().map(|table| table_json(table)).collect())
        }
    }
}

fn value_json(value: &Value) -> Json {
    match value {
        Value::String(text) => Json::String(text.value().clone()),
        Value::Integer(number) => Json::from(*number.value()),
        Value::Float(number) => {
            serde_json::Number::from_f64(*number.value()).map_or(Json::Null, Json::Number)
        }
        Value::Boolean(flag) => Json::Bool(*flag.value()),
        Value::Datetime(datetime) => Json::String(datetime.value().to_string()),
        Value::Array(array) => Json::Array(array.iter().map(value_json).collect()),
        Value::InlineTable(table) => table_json(table),
    }
}

fn table_json(table: &dyn TableLike) -> Json {
    Json::Object(
        table
            .iter()
            .map(|(key, item)| (key.to_owned(), to_json(item)))
            .collect(),
    )
}

/// The TOML item `value` writes: an object as a standard table, the way a
/// person writes one, anything else as a value. `null`, which TOML cannot
/// hold, is refused naming where it sits, and so is an integer past TOML's
/// range.
pub(crate) fn to_item(value: &Json) -> Result<Item, String> {
    Ok(match toml_value(value, "")? {
        Value::InlineTable(table) => {
            let mut item = standard_table(table.into_table());
            if let Item::Table(table) = &mut item {
                show_empty_tables(table);
            }
            item
        }
        value => Item::Value(value),
    })
}

/// Gives each empty table in `table` its header: one left implicit is not
/// written at all, so an empty entry, or an empty field such as `env = {}`,
/// would vanish from the file.
fn show_empty_tables(table: &mut toml_edit::Table) {
    if table.is_empty() {
        table.set_implicit(false);
    }
    for (_, item) in table.iter_mut() {
        if let Item::Table(child) = item {
            show_empty_tables(child);
        }
    }
}

/// `value`, which sits at the dotted path `at` of an entry, as a TOML
/// value. Built by hand: through serde, a JSON number would arrive as the
/// private map `serde_json`'s `arbitrary_precision` makes of it.
fn toml_value(value: &Json, at: &str) -> Result<Value, String> {
    let child = |key: &str| match at {
        "" => key.to_owned(),
        at => format!("{at}.{key}"),
    };
    Ok(match value {
        Json::Null if at.is_empty() => return Err("must not be null".to_owned()),
        Json::Null => return Err(format!("`{at}` is null, which TOML cannot hold")),
        Json::Bool(flag) => Value::from(*flag),
        // `arbitrary_precision` keeps the number's text, which says whether
        // it is a float: an integer past `i64` would otherwise parse as one
        // and lose its digits.
        Json::Number(number) => {
            let float = number.to_string().contains(['.', 'e', 'E']);
            match (number.as_i64(), number.as_f64()) {
                (Some(integer), _) => Value::from(integer),
                (None, Some(value)) if float => Value::from(value),
                (None, None) if float => {
                    return Err(format!("{number} is past TOML's float range"));
                }
                _ => return Err(format!("{number} is past TOML's integer range")),
            }
        }
        Json::String(text) => Value::from(text.as_str()),
        Json::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| toml_value(item, &child(&index.to_string())))
                .collect::<Result<_, _>>()?,
        ),
        Json::Object(fields) => {
            let mut table = InlineTable::new();
            for (key, field) in fields {
                table.insert(key, toml_value(field, &child(key))?);
            }
            Value::InlineTable(table)
        }
    })
}

/// The entries of the table at the key path `table`, by key: each key's item
/// as JSON. Empty when the file sets no table there.
pub(crate) fn keyed_entries(doc: &ConfigDocument, table: &[&str]) -> BTreeMap<String, Json> {
    doc.get_at(table)
        .and_then(Item::as_table_like)
        .map(|entries| {
            entries
                .iter()
                .map(|(key, item)| (key.to_owned(), to_json(item)))
                .collect()
        })
        .unwrap_or_default()
}

/// The key path of entry `key` of the table at `table`.
pub(crate) fn entry_path<'a>(table: &[&'a str], key: &'a str) -> Vec<&'a str> {
    table.iter().copied().chain([key]).collect()
}

/// The key path `path` as `config.toml` would write it, each segment quoted
/// when it needs to be: `providers.custom."my.box"`. Errors name entries by
/// it.
pub(crate) fn dotted_key(path: &[&str]) -> String {
    path.iter()
        .map(|segment| toml_edit::Key::new(*segment).display_repr().into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

/// `value` as canonical JSON bytes: every object's keys sorted, whatever
/// order the file wrote them in, so equal entries give equal bytes.
pub(crate) fn canonical_json<T: Serialize>(value: &T) -> Vec<u8> {
    let value = serde_json::to_value(value).expect("table entries encode as JSON");
    serde_json::to_vec(&sorted(value)).expect("JSON values encode")
}

fn sorted(value: Json) -> Json {
    match value {
        Json::Object(fields) => {
            let fields = fields.into_iter().collect::<BTreeMap<_, _>>();
            Json::Object(
                fields
                    .into_iter()
                    .map(|(key, field)| (key, sorted(field)))
                    .collect::<Map<_, _>>(),
            )
        }
        Json::Array(items) => Json::Array(items.into_iter().map(sorted).collect()),
        scalar => scalar,
    }
}

/// The revision of what a read returned: the SHA-256 of its
/// [`canonical_json`], so it survives a daemon restart and a hand edit that
/// changes only comments or formatting. Not for values a client must not
/// learn by guessing, such as secrets, which need a keyed digest over
/// [`canonical_json`] instead.
pub(crate) fn revision<T: Serialize>(value: &T) -> ConfigRevision {
    ConfigRevision::new(hex::encode(Sha256::digest(canonical_json(value))))
}

/// Why changes to `config.toml` were not applied; the document is left as
/// it was.
#[derive(Debug, PartialEq)]
pub(crate) enum ChangeError {
    /// A change the setting or entry refuses, an unknown key, or a result
    /// that would not load, such as an alias the conversation model names
    /// removed.
    Refused(Vec<ConfigError>),
    /// The file does not load for keys the changes leave alone; a hand
    /// edit must fix it.
    FileInvalid(Vec<ConfigError>),
}

/// Loads `edited`, the file `original` held with the entries at the key
/// paths `changed` put or removed, as the daemon would load it, before
/// anything is saved: the config it loads as, or why the changes are
/// refused. When `original` does not load either, the fault is the changes'
/// only if `original` loads without those entries; otherwise it lies in
/// keys they leave alone. A file that did not load is saved once the changes
/// repair it.
pub(crate) fn load_edited(
    original: &ConfigDocument,
    edited: &ConfigDocument,
    changed: &[Vec<&str>],
) -> Result<(Config, Vec<ConfigWarning>), ChangeError> {
    load::resolve(edited).map_err(|errors| {
        if load::resolve(original).is_ok() {
            return ChangeError::Refused(errors);
        }
        let mut untouched = original.clone();
        for path in changed {
            untouched.remove_at(path);
        }
        match load::resolve(&untouched) {
            Ok(_) => ChangeError::Refused(errors),
            Err(_) => ChangeError::FileInvalid(errors),
        }
    })
}

#[cfg(test)]
#[path = "config_entries_tests.rs"]
mod tests;
