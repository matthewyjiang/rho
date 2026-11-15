//! The catalogs a renderer supports, with the functions that implement them,
//! and the v1.0 rule that resolves the catalog an item runs in.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::catalog::{Catalog, CatalogError};
use crate::functions::Functions;
use crate::validate::CatalogValidator;

/// Why the catalog of a component or function call cannot be resolved on a
/// surface. There is no fallback to another catalog.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UnresolvedCatalog {
    /// The item names no catalog and the surface has no default one.
    #[error("surface {surface:?} has no default catalogId, and the item names none")]
    NoDefault {
        /// The surface.
        surface: String,
    },
    /// The item, or the surface, names a catalog this renderer lacks.
    #[error("Catalog not found: {0}")]
    NotFound(String),
}

/// One supported catalog: its declarations, the functions that implement
/// them, and its validator, compiled once.
#[derive(Debug)]
pub(crate) struct Supported {
    pub(crate) catalog: Catalog,
    pub(crate) functions: Functions,
    pub(crate) validator: CatalogValidator,
}

/// The supported catalogs, by id.
#[derive(Debug, Default)]
pub(crate) struct Catalogs {
    entries: BTreeMap<String, Supported>,
}

impl Catalogs {
    /// Adds `catalog`, replacing one with the same id.
    pub(crate) fn add(
        &mut self,
        catalog: Catalog,
        functions: Functions,
    ) -> Result<(), CatalogError> {
        let validator = CatalogValidator::new(&catalog)?;
        self.entries.insert(
            catalog.id().to_owned(),
            Supported {
                catalog,
                functions,
                validator,
            },
        );
        Ok(())
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Supported> {
        self.entries.get(id)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Supported> {
        self.entries.values()
    }

    /// The catalog an item runs in on `surface`, with its id: the one the
    /// item names, else the surface's default.
    pub(crate) fn resolve(
        &self,
        named: Option<&str>,
        default: Option<&str>,
        surface: &str,
    ) -> Result<(&str, &Supported), UnresolvedCatalog> {
        let id = named
            .or(default)
            .ok_or_else(|| UnresolvedCatalog::NoDefault {
                surface: surface.to_owned(),
            })?;
        let (id, supported) = self
            .entries
            .get_key_value(id)
            .ok_or_else(|| UnresolvedCatalog::NotFound(id.to_owned()))?;
        Ok((id, supported))
    }
}
