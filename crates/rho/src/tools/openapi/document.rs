//! The parsed spec document, JSON pointers into it, and local `$ref` chains.

use std::{borrow::Cow, collections::HashSet, fmt};

use percent_encoding::percent_decode_str;
use serde_json::{Map, Value};

use super::LoadError;

/// A ref may point at another ref; this caps how many hops are followed.
/// GitHub's 1,792 distinct refs chain at most 1 hop, so a longer chain is a
/// malformed spec rather than a real one.
const MAX_REF_HOPS: usize = 16;

/// The OpenAPI version, which decides how schemas are normalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecVersion {
    /// OpenAPI 3.0: schemas use `nullable`, boolean exclusive bounds, and
    /// `$ref` siblings are ignored.
    V3_0,
    /// OpenAPI 3.1: schemas are JSON Schema 2020-12.
    V3_1,
}

impl SpecVersion {
    /// OpenAPI 3.0 ignores every sibling of `$ref`; 3.1 applies them.
    pub(super) fn ignores_ref_siblings(self) -> bool {
        match self {
            Self::V3_0 => true,
            Self::V3_1 => false,
        }
    }
}

/// A parsed spec. Read-only after load and shared behind an `Arc`.
#[derive(Debug)]
pub(crate) struct Document {
    version: SpecVersion,
    root: Value,
}

/// An RFC 6901 pointer into the document, such as `/components/schemas/user`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct JsonPointer(String);

/// Why a `$ref` could not be followed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RefError {
    #[error("external $ref '{0}' is not supported")]
    External(String),
    #[error("$ref '{0}' points at nothing")]
    Broken(String),
    #[error("$ref '{0}' is part of a reference cycle")]
    Cycle(String),
    #[error("$ref '{reference}' chains more than {limit} references")]
    TooDeep { reference: String, limit: usize },
    #[error("$ref '{reference}' is one of more than {limit} schemas joined through allOf")]
    TooManyAllOf { reference: String, limit: usize },
}

impl Document {
    /// Parse JSON or YAML and check the version.
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, LoadError> {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let looks_like_json = bytes
            .iter()
            .find(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|byte| *byte == b'{');
        let root: Value = if looks_like_json {
            serde_json::from_slice(bytes).map_err(LoadError::Json)?
        } else {
            super::yaml::parse(bytes)?
        };
        let version = detect_version(&root)?;
        Ok(Self { version, root })
    }

    pub(super) fn version(&self) -> SpecVersion {
        self.version
    }

    pub(super) fn root(&self) -> &Value {
        &self.root
    }

    /// The value at `pointer`, if any.
    pub(super) fn get(&self, pointer: &JsonPointer) -> Option<&Value> {
        self.root.pointer(&pointer.0)
    }

    /// Follow a `$ref` through any chain of further refs to the first value
    /// that is not a reference, and return where that value lives. Returns
    /// `None` when `value` is not a reference.
    pub(super) fn resolve<'a>(
        &'a self,
        mut value: &'a Value,
    ) -> Result<Option<(JsonPointer, &'a Value)>, RefError> {
        let mut seen = HashSet::new();
        let mut at = None;
        while let Some(reference) = ref_of(value) {
            if seen.len() == MAX_REF_HOPS {
                return Err(RefError::TooDeep {
                    reference: reference.to_owned(),
                    limit: MAX_REF_HOPS,
                });
            }
            let target = JsonPointer::from_ref(reference)?;
            if !seen.insert(target.clone()) {
                return Err(RefError::Cycle(reference.to_owned()));
            }
            value = self
                .get(&target)
                .ok_or_else(|| RefError::Broken(reference.to_owned()))?;
            at = Some(target);
        }
        Ok(at.map(|at| (at, value)))
    }

    /// Where the `$ref` of a schema points, after checking that its whole
    /// chain resolves. OpenAPI 3.0 ignores `$ref` siblings, so a chain
    /// collapses to its end; in 3.1 every link may add constraints, so only
    /// the first hop is taken. Returns `None` when `schema` has no `$ref`.
    pub(super) fn schema_ref_target(
        &self,
        schema: &Map<String, Value>,
    ) -> Result<Option<JsonPointer>, RefError> {
        let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
            return Ok(None);
        };
        let first = JsonPointer::from_ref(reference)?;
        let value = self
            .get(&first)
            .ok_or_else(|| RefError::Broken(reference.to_owned()))?;
        let last = self
            .resolve(value)?
            .map_or_else(|| first.clone(), |(at, _)| at);
        Ok(Some(if self.version.ignores_ref_siblings() {
            last
        } else {
            first
        }))
    }

    /// The value at `at` with any `$ref` chain followed, and where it lives.
    pub(super) fn resolve_at(&self, at: JsonPointer) -> Result<(JsonPointer, &Value), RefError> {
        let value = self
            .get(&at)
            .ok_or_else(|| RefError::Broken(at.to_string()))?;
        Ok(self.resolve(value)?.unwrap_or((at, value)))
    }
}

/// The `$ref` string of a reference object.
fn ref_of(value: &Value) -> Option<&str> {
    value.get("$ref")?.as_str()
}

fn detect_version(root: &Value) -> Result<SpecVersion, LoadError> {
    if root.get("swagger").is_some() {
        return Err(LoadError::Swagger2);
    }
    // YAML reads an unquoted `openapi: 3.0` as a number.
    let version = match root.get("openapi") {
        Some(Value::String(version)) => Cow::Borrowed(version.as_str()),
        Some(Value::Number(version)) => Cow::Owned(version.to_string()),
        _ => return Err(LoadError::MissingVersion),
    };
    let mut parts = version.split('.');
    match (parts.next(), parts.next()) {
        (Some("3"), Some("0")) => Ok(SpecVersion::V3_0),
        (Some("3"), Some("1")) => Ok(SpecVersion::V3_1),
        _ => Err(LoadError::UnsupportedVersion(version.into_owned())),
    }
}

impl JsonPointer {
    /// The whole document.
    pub(super) fn root() -> Self {
        Self(String::new())
    }

    /// Parse a local `$ref` such as `#/components/schemas/user`. A ref is a
    /// URI fragment, so it may be percent-encoded.
    pub(super) fn from_ref(reference: &str) -> Result<Self, RefError> {
        let Some(fragment) = reference.strip_prefix('#') else {
            return Err(RefError::External(reference.to_owned()));
        };
        let pointer = percent_decode_str(fragment)
            .decode_utf8()
            .map_err(|_| RefError::Broken(reference.to_owned()))?;
        if !pointer.is_empty() && !pointer.starts_with('/') {
            // `#name` is a JSON Schema anchor, not a pointer.
            return Err(RefError::Broken(reference.to_owned()));
        }
        Ok(Self(pointer.into_owned()))
    }

    /// The pointer to child `segment`, escaped per RFC 6901.
    pub(super) fn child(&self, segment: &str) -> Self {
        let escaped = segment.replace('~', "~0").replace('/', "~1");
        Self(format!("{}/{escaped}", self.0))
    }

    /// The pointer to array element `index`.
    pub(super) fn index(&self, index: usize) -> Self {
        Self(format!("{}/{index}", self.0))
    }

    /// The pointer's unescaped segments.
    pub(super) fn segments(&self) -> impl Iterator<Item = String> + '_ {
        self.0
            .split('/')
            .skip(1)
            .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
    }
}

impl fmt::Display for JsonPointer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{}", self.0)
    }
}
