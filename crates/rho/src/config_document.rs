//! `config.toml` held as a `toml_edit` document, so an edit changes only the
//! keys it names and keeps the user's comments, key order, and formatting.
//!
//! Keys are dotted paths (`web_search.openai.connection`). Reads and writes
//! walk standard tables, dotted keys, and inline tables alike.

use std::path::Path;

use anyhow::Context;
use serde::{de::DeserializeOwned, Serialize};
use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike, Value};

/// A parsed TOML file that keeps everything the user wrote.
#[derive(Clone, Debug, Default)]
pub(crate) struct ConfigDocument {
    doc: DocumentMut,
}

impl ConfigDocument {
    /// Reads `path`. A missing file is an empty document; reading never
    /// creates one.
    pub(crate) fn read(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .with_context(|| format!("failed to parse config file {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("failed to read config file {}", path.display()))
            }
        }
    }

    pub(crate) fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(Self {
            doc: text.parse::<DocumentMut>()?,
        })
    }

    /// The file text this document writes.
    pub(crate) fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Replaces `path` atomically with this document.
    pub(crate) fn write(&self, path: &Path) -> anyhow::Result<()> {
        crate::config_writer::write_bytes_atomically(path, self.doc.to_string().as_bytes())
            .with_context(|| format!("failed to write config file {}", path.display()))
    }

    /// The item at `key`, if the file sets it.
    pub(crate) fn get(&self, key: &str) -> Option<&Item> {
        let mut segments = key.split('.');
        let mut item = self.doc.as_table().get(segments.next()?)?;
        for segment in segments {
            item = item.as_table_like()?.get(segment)?;
        }
        (!item.is_none()).then_some(item)
    }

    /// Sets the value at `key`, creating parent tables as needed. Replacing
    /// a value keeps its comments; an equal value is left as written.
    pub(crate) fn set_value(&mut self, key: &str, value: Value) {
        let (parent, leaf) = split_key(key);
        let table = self.table_mut(&parent);
        match table.get_mut(leaf) {
            Some(Item::Value(old)) => replace_value(old, value),
            Some(slot) => *slot = Item::Value(value),
            None => {
                table.insert(leaf, Item::Value(value));
            }
        }
    }

    /// Writes a structured item at `key`, editing only the entries that
    /// differ so unchanged ones keep their comments. `None` removes it.
    pub(crate) fn set_item(&mut self, key: &str, new: Option<Item>) {
        let Some(new) = new else {
            self.remove(key);
            return;
        };
        let (parent, leaf) = split_key(key);
        let table = self.table_mut(&parent);
        match table.get_mut(leaf) {
            Some(existing) => merge_item(existing, new),
            None => {
                table.insert(leaf, new);
            }
        }
    }

    /// Removes `key` with its own comments, then any parent table the
    /// removal left empty, unless that table carries a comment.
    pub(crate) fn remove(&mut self, key: &str) {
        let segments = key.split('.').collect::<Vec<_>>();
        remove_path(self.doc.as_table_mut(), &segments);
    }

    /// Every key path that holds a value or an array of tables, in file
    /// order. Tables themselves are walked, not listed.
    pub(crate) fn leaf_paths(&self) -> Vec<Vec<String>> {
        let mut leaves = Vec::new();
        collect_leaves(self.doc.as_table(), &mut Vec::new(), &mut leaves);
        leaves
    }

    /// The table at `key`, creating implicit tables on the way. A non-table
    /// item in the way is replaced.
    fn table_mut(&mut self, path: &[&str]) -> &mut dyn TableLike {
        let mut table: &mut dyn TableLike = self.doc.as_table_mut();
        let mut inline = false;
        for segment in path {
            let slot = table.entry(segment).or_insert(Item::None);
            if !slot.is_table_like() {
                *slot = empty_table(inline);
            }
            inline = inline || slot.is_inline_table();
            table = slot
                .as_table_like_mut()
                .expect("the slot was just made a table");
        }
        table
    }
}

impl From<DocumentMut> for ConfigDocument {
    fn from(doc: DocumentMut) -> Self {
        Self { doc }
    }
}

impl From<ConfigDocument> for DocumentMut {
    fn from(doc: ConfigDocument) -> Self {
        doc.doc
    }
}

/// Decodes a structured table into its serde type. The error names the
/// problem, not the key.
pub(crate) fn decode_table<T: DeserializeOwned>(item: &Item) -> Result<T, String> {
    let table = item
        .clone()
        .into_table()
        .map_err(|_| "must be a table".to_owned())?;
    toml_edit::de::from_document(DocumentMut::from(table))
        .map_err(|error| error.to_string().trim_end().to_owned())
}

/// Encodes a serde value as a standard table, the way a person would write
/// it: nested tables get their own `[header]` instead of `{ inline }` form,
/// and a table holding only tables gets no header of its own.
pub(crate) fn encode_table<T: Serialize>(value: &T) -> anyhow::Result<Item> {
    let mut table = toml_edit::ser::to_document(value)?.into_table();
    expand_inline_tables(&mut table);
    table.set_implicit(true);
    Ok(Item::Table(table))
}

/// The value as written, without surrounding whitespace or comments.
pub(crate) fn shown(value: &Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

fn split_key(key: &str) -> (Vec<&str>, &str) {
    let mut segments = key.split('.').collect::<Vec<_>>();
    let leaf = segments.pop().unwrap_or(key);
    (segments, leaf)
}

fn empty_table(inline: bool) -> Item {
    if inline {
        return Item::Value(Value::InlineTable(InlineTable::new()));
    }
    let mut table = Table::new();
    table.set_implicit(true);
    Item::Table(table)
}

fn replace_value(old: &mut Value, new: Value) {
    if same_value(old, &new) {
        return;
    }
    let decor = old.decor().clone();
    *old = new;
    *old.decor_mut() = decor;
}

/// Equal values compare equal however they are quoted or spaced.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a.value() == b.value(),
        (Value::Integer(a), Value::Integer(b)) => a.value() == b.value(),
        (Value::Boolean(a), Value::Boolean(b)) => a.value() == b.value(),
        _ => shown(a) == shown(b),
    }
}

fn merge_item(existing: &mut Item, new: Item) {
    match new {
        Item::Value(new) => {
            if let Item::Value(old) = existing {
                replace_value(old, new);
                return;
            }
            *existing = Item::Value(new);
        }
        Item::Table(new) => {
            if let Some(old) = existing.as_table_like_mut() {
                merge_into(old, new);
                return;
            }
            *existing = Item::Table(new);
        }
        other => *existing = other,
    }
}

fn merge_into(old: &mut dyn TableLike, new: Table) {
    let stale = old
        .iter()
        .map(|(key, _)| key.to_owned())
        .filter(|key| !new.contains_key(key))
        .collect::<Vec<_>>();
    for key in stale {
        old.remove(&key);
    }
    for (key, item) in new {
        match old.get_mut(&key) {
            Some(slot) => merge_item(slot, item),
            None => {
                old.insert(&key, item);
            }
        }
    }
}

fn remove_path(table: &mut dyn TableLike, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        table.remove(first);
        return;
    }
    let Some(child) = table.get_mut(first) else {
        return;
    };
    let Some(child_table) = child.as_table_like_mut() else {
        return;
    };
    remove_path(child_table, rest);
    if child_table.is_empty() && carries_no_comment(child) {
        table.remove(first);
    }
}

fn carries_no_comment(item: &Item) -> bool {
    let decor = match item {
        Item::Table(table) => table.decor(),
        Item::Value(value) => value.decor(),
        Item::None | Item::ArrayOfTables(_) => return false,
    };
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .all(|raw| raw.as_str().is_some_and(|text| text.trim().is_empty()))
}

fn collect_leaves(table: &dyn TableLike, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    for (key, item) in table.iter() {
        prefix.push(key.to_owned());
        match item.as_table_like() {
            Some(child) => collect_leaves(child, prefix, out),
            None => out.push(prefix.clone()),
        }
        prefix.pop();
    }
}

fn expand_inline_tables(table: &mut Table) {
    for (_, item) in table.iter_mut() {
        if item.is_inline_table() {
            *item = std::mem::take(item)
                .into_table()
                .map_or_else(|item| item, Item::Table);
        }
        if let Item::Table(child) = item {
            child.set_implicit(true);
            expand_inline_tables(child);
        }
    }
}

#[cfg(test)]
#[path = "config_document_tests.rs"]
mod tests;
