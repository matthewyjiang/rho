//! A2UI Express, the compact notation a model writes a surface in: its
//! parser, its compiler to v1.0 messages, its decompiler, and the prompt
//! that teaches it.

use std::collections::{BTreeMap, HashSet};

use crate::catalog::Catalog;
use crate::validate::CatalogValidator;

mod compile;
mod decompile;
mod error;
mod lexer;
mod parser;
mod prompt;
mod signature;

pub use error::{ErrorKind, ExpressError, Position};
pub use prompt::{AllowedMessage, Framing, PromptOptions};

use signature::{FunctionSignature, Signatures};

/// The surface id a block compiles against when it names none and the
/// caller has no better one.
pub const DEFAULT_SURFACE_ID: &str = "default_surface";

/// The protocol version Express compiles to.
const VERSION: &str = "1.0";

/// The Express format for a set of catalogs: compiles blocks into A2UI v1.0
/// messages, writes messages back as Express, and renders the prompt that
/// teaches a model the notation.
///
/// Building one reads every catalog's signatures and compiles its
/// validation schema, so build it once per set of catalogs and keep it.
///
/// With one catalog, a compiled `createSurface` names it and components do
/// not. With several, `createSurface` names none and every component and
/// function call names its own, found by looking its name up across the
/// catalogs; a name several catalogs define must say which with
/// `catalogId=`.
#[derive(Debug, Clone)]
pub struct Express {
    catalogs: Vec<Catalog>,
    signatures: Vec<Signatures>,
    validators: Vec<CatalogValidator>,
    system: BTreeMap<String, FunctionSignature>,
}

impl Express {
    /// Prepares the format for `catalogs`, the default catalog first.
    ///
    /// # Errors
    ///
    /// An [`ErrorKind::Catalog`] error when there is no catalog, two share
    /// an id, or a catalog's schemas do not compile.
    pub fn new(catalogs: Vec<Catalog>) -> Result<Self, ExpressError> {
        if catalogs.is_empty() {
            return Err(ExpressError::catalog(
                "Express needs at least one catalog to describe",
            ));
        }
        let mut seen = HashSet::new();
        if let Some(catalog) = catalogs.iter().find(|c| !seen.insert(c.id())) {
            return Err(ExpressError::catalog(format!(
                "two catalogs have the id {:?}",
                catalog.id()
            )));
        }
        let validators = catalogs
            .iter()
            .map(CatalogValidator::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ExpressError::catalog(e.to_string()))?;
        Ok(Self {
            signatures: catalogs.iter().map(Signatures::new).collect(),
            catalogs,
            validators,
            system: signature::system_functions(),
        })
    }

    /// The catalogs, in the order given.
    pub fn catalogs(&self) -> &[Catalog] {
        &self.catalogs
    }

    /// The functions the agent may call on its own with a standalone call,
    /// which compiles to `callRendererFunction`: those whose
    /// `allowedCallers` admits the agent and that need no user activation,
    /// in catalog order.
    pub fn agent_callable_functions(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for signatures in &self.signatures {
            for name in &signatures.function_order {
                let callable = signatures
                    .functions
                    .get(name)
                    .is_some_and(FunctionSignature::agent_callable);
                if callable && !names.contains(&name.as_str()) {
                    names.push(name);
                }
            }
        }
        names
    }

    fn is_multi_catalog(&self) -> bool {
        self.catalogs.len() > 1
    }

    /// The index of the catalog with id `id`.
    fn catalog_index(&self, id: &str) -> Option<usize> {
        self.catalogs.iter().position(|c| c.id() == id)
    }
}
