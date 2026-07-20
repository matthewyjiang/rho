//! Google Gemini `generateContent` wire contract and canonical conversion.
//!
//! Authentication, endpoints, model discovery, and provider policy belong to
//! `providers::google`; this module only owns Google request and response data.

mod convert;
mod stream;
mod types;

pub(crate) use convert::{build_request, ResponseCollector};
pub(crate) use stream::collect_stream;
pub(crate) use types::*;

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
