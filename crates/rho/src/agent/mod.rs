//! Portable agent definitions and deterministic catalog discovery.
//!
//! Definitions contain only semantic policy. Origin and source paths remain
//! catalog metadata and do not affect semantic fingerprints.

mod catalog;
mod definition;
mod internal;
mod one_shot;
mod parser;

pub(crate) use catalog::*;
pub(crate) use definition::*;
pub(crate) use internal::*;
pub(crate) use one_shot::*;
#[cfg(test)]
pub(crate) use parser::parse_definition;

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
