//! OpenAPI 3.0 and 3.1 spec loading for generated REST tools.
//!
//! [`OpenApiSpec::load`] parses a spec once, keeps it behind an [`Arc`], and
//! indexes its operations without rendering any schema. Operations the tools
//! cannot call correctly are skipped with a [`operation::SkipReason`]. Only a
//! parse error, an unsupported version, or an oversized spec fails the whole
//! source. Schemas render on demand through [`Document::render_schema`].

use std::sync::Arc;

use url::Url;

pub(crate) mod document;
pub(crate) mod operation;
pub(crate) mod params;
pub(crate) mod schema;
pub(crate) mod servers;
mod yaml;

use document::Document;
use operation::{Operation, SkippedOperation};
use servers::Servers;

/// Default `max_spec_bytes`. The largest public specs measured are
/// Cloudflare's at 27.5 MB and GitHub's at 13.5 MB, so this leaves more than
/// 2x headroom while still refusing a runaway download. Parsed, GitHub's spec
/// held about 100 MB, so memory grows at roughly 7x the spec size.
pub(crate) const DEFAULT_MAX_SPEC_BYTES: usize = 64 * 1024 * 1024;

/// One spec to load.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpecInput<'a> {
    /// Config name of the source. Tool names are `openapi__<name>__<operation>`.
    pub(crate) name: &'a str,
    /// Where the spec came from. Relative server URLs resolve against it.
    pub(crate) url: &'a Url,
    /// Raw JSON or YAML.
    pub(crate) bytes: &'a [u8],
    /// Specs larger than this fail with [`LoadError::TooLarge`].
    pub(crate) max_spec_bytes: usize,
}

/// A parsed spec and its operation index.
#[derive(Debug)]
pub(crate) struct OpenApiSpec {
    /// The parsed document, shared with every tool built from it.
    pub(crate) document: Arc<Document>,
    /// Resolved `servers`, in spec order. A spec without servers gets `/`.
    pub(crate) servers: Servers,
    /// Callable operations, in document order.
    pub(crate) operations: Vec<Operation>,
    /// Operations left out, each with the reason.
    pub(crate) skipped: Vec<SkippedOperation>,
}

/// Why a whole spec failed to load.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LoadError {
    #[error("OpenAPI spec is {size} bytes, over max_spec_bytes ({limit} bytes)")]
    TooLarge { size: usize, limit: usize },
    #[error("could not parse OpenAPI spec as JSON: {0}")]
    Json(serde_json::Error),
    #[error("could not parse OpenAPI spec as YAML: {0}")]
    Yaml(serde_yaml_ng::Error),
    #[error("YAML spec expands past {limit} nodes (one per byte of the spec); its aliases repeat too much")]
    YamlTooManyNodes { limit: usize },
    #[error("Swagger 2.0 specs are not supported; convert the spec to OpenAPI 3")]
    Swagger2,
    #[error("OpenAPI version {0} is not supported; expected 3.0 or 3.1")]
    UnsupportedVersion(String),
    #[error("document is not an OpenAPI spec: it has no `openapi` version")]
    MissingVersion,
}

impl OpenApiSpec {
    /// Parse and index one spec.
    pub(crate) fn load(input: SpecInput<'_>) -> Result<Self, LoadError> {
        if input.bytes.len() > input.max_spec_bytes {
            return Err(LoadError::TooLarge {
                size: input.bytes.len(),
                limit: input.max_spec_bytes,
            });
        }
        let document = Document::parse(input.bytes)?;
        let servers = servers::resolve_spec(document.root(), input.url);
        let (operations, skipped) = operation::index(&document, input.name, input.url);
        Ok(Self {
            document: Arc::new(document),
            servers,
            operations,
            skipped,
        })
    }
}

#[cfg(test)]
#[path = "openapi_tests.rs"]
mod tests;
