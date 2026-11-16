//! Pruning a catalog to an allowlist of components or functions, as an
//! agent does before it prompts with the catalog or validates against it.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::catalog::{union, Catalog, CatalogError, EntryKind};

impl Catalog {
    /// The catalog with only the components `names` lists.
    ///
    /// The allowlist is read literally: an empty one keeps no component, a
    /// name the catalog does not declare is ignored, and a repeated name
    /// counts once. Functions, the id and everything else are kept, and
    /// the `anyComponent` union lists only what is kept. To keep every
    /// component, do not prune.
    ///
    /// # Errors
    ///
    /// Fails as [`Catalog::from_json`] does when a kept definition refers
    /// to a pruned one with a `$ref`.
    pub fn keep_components<S: AsRef<str>>(&self, names: &[S]) -> Result<Self, CatalogError> {
        self.keep(EntryKind::Component, names)
    }

    /// The catalog with only the functions `names` lists, read as
    /// [`Catalog::keep_components`] reads its allowlist.
    ///
    /// # Errors
    ///
    /// Fails as [`Catalog::from_json`] does when a kept definition refers
    /// to a pruned one with a `$ref`.
    pub fn keep_functions<S: AsRef<str>>(&self, names: &[S]) -> Result<Self, CatalogError> {
        self.keep(EntryKind::Function, names)
    }

    fn keep<S: AsRef<str>>(&self, kind: EntryKind, names: &[S]) -> Result<Self, CatalogError> {
        let allowed: BTreeSet<&str> = names.iter().map(AsRef::as_ref).collect();
        let (entries_key, union_key) = match kind {
            EntryKind::Component => ("components", "anyComponent"),
            EntryKind::Function => ("functions", "anyFunction"),
        };
        let kept: Vec<&str> = self
            .ordered_names(kind)
            .into_iter()
            .filter(|name| allowed.contains(name))
            .collect();
        let mut document = self.to_json();
        if let Some(Value::Object(entries)) = document.get_mut(entries_key) {
            entries.retain(|name, _| kept.contains(&name.as_str()));
        }
        if let Some(Value::Object(defs)) = document.get_mut("$defs") {
            if defs.contains_key(union_key) {
                defs.insert(union_key.to_owned(), union(kind, kept.iter().copied()));
            }
        }
        Self::from_json(document)
    }
}
