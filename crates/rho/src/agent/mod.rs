//! Portable agent definitions and deterministic catalog discovery.
//!
//! Definitions contain only semantic policy. Origin and source paths remain
//! catalog metadata and do not affect semantic fingerprints.

mod catalog;
mod definition;
mod parser;

pub(crate) use catalog::*;
pub(crate) use definition::*;
#[cfg(test)]
pub(crate) use parser::parse_definition;

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
