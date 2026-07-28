//! Portable agent definitions and deterministic catalog discovery.
//!
//! Definitions contain only semantic policy. Origin and source paths remain
//! catalog metadata and do not affect semantic fingerprints.

mod catalog;
mod definition;
mod edit;
mod internal;
mod one_shot;
mod parser;
mod serializer;

pub(crate) use catalog::*;
pub(crate) use definition::*;
pub(crate) use edit::{save_definition, SaveDefinitionError};
pub(crate) use internal::*;
pub(crate) use one_shot::*;
pub(crate) use parser::{parse_definition, parse_tools_list_text};
pub(crate) use rho_providers::reasoning::ReasoningLevel;
pub(crate) use serializer::serialize_definition;

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
