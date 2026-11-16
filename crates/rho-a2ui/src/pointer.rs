use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Segments that no path may contain, in every language, so that one payload
/// behaves the same everywhere.
const FORBIDDEN_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// A normalized absolute JSON Pointer into a data model.
///
/// Parsing splits on `/` and drops empty segments, so `//a///b//`, `/a/b/`
/// and `/a/b` are equal, and `""` and `/` are the root. Segments are stored
/// decoded: `~1` is `/` and `~0` is `~`, in that order (RFC 6901).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pointer {
    segments: Vec<String>,
}

/// Why a string is not a [`Pointer`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PointerError {
    /// A segment is `__proto__`, `constructor` or `prototype`.
    #[error("forbidden path segment {segment:?} in {path:?}")]
    ForbiddenSegment {
        /// The path that was being parsed.
        path: String,
        /// The decoded segment.
        segment: String,
    },
    /// A `~` not followed by `0` or `1`.
    #[error("invalid escape in path {path:?}: '~' must be followed by '0' or '1'")]
    InvalidEscape {
        /// The path as written.
        path: String,
    },
    /// A path from the wire that is neither empty nor starts with `/`.
    #[error("path {path:?} is not a JSON Pointer: it must start with '/'")]
    NotAbsolute {
        /// The path as written.
        path: String,
    },
}

impl Pointer {
    /// The root pointer, `/`.
    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    /// Parses `path` as a normalized absolute pointer.
    ///
    /// # Errors
    ///
    /// Returns [`PointerError::ForbiddenSegment`] when a segment is
    /// `__proto__`, `constructor` or `prototype`.
    pub fn parse(path: &str) -> Result<Self, PointerError> {
        let mut segments = Vec::new();
        for raw in path.split('/').filter(|raw| !raw.is_empty()) {
            let segment = raw.replace("~1", "/").replace("~0", "~");
            if FORBIDDEN_SEGMENTS.contains(&segment.as_str()) {
                return Err(PointerError::ForbiddenSegment {
                    path: path.to_owned(),
                    segment,
                });
            }
            segments.push(segment);
        }
        Ok(Self { segments })
    }

    /// Parses a JSON Pointer as the wire writes one, in `updateDataModel`:
    /// empty or starting with `/`, with every `~` escaping `0` or `1`.
    ///
    /// # Errors
    ///
    /// Returns [`PointerError::NotAbsolute`], [`PointerError::InvalidEscape`]
    /// or, as [`Pointer::parse`] does, [`PointerError::ForbiddenSegment`].
    pub fn from_wire(path: &str) -> Result<Self, PointerError> {
        if !path.is_empty() && !path.starts_with('/') {
            return Err(PointerError::NotAbsolute {
                path: path.to_owned(),
            });
        }
        check_escapes(path)?;
        Self::parse(path)
    }

    /// The decoded segments, outermost first.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Whether this is the root pointer.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// The path one segment up, or `None` for the root.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let (_, parents) = self.segments.split_last()?;
        Some(Self {
            segments: parents.to_vec(),
        })
    }

    /// Whether `self` is `other` or a path above it.
    #[must_use]
    pub fn is_prefix_of(&self, other: &Self) -> bool {
        other.segments.starts_with(&self.segments)
    }

    /// The first `len` segments of `self`, which are already checked.
    pub(crate) fn truncated(&self, len: usize) -> Self {
        Self {
            segments: self.segments[..len].to_vec(),
        }
    }
}

impl FromStr for Pointer {
    type Err = PointerError;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        Self::parse(path)
    }
}

impl fmt::Display for Pointer {
    /// Writes the canonical form with escapes, and `/` for the root.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.segments.is_empty() {
            return f.write_str("/");
        }
        for segment in &self.segments {
            write!(f, "/{}", segment.replace('~', "~0").replace('/', "~1"))?;
        }
        Ok(())
    }
}

/// Checks that every `~` in `path` starts an RFC 6901 escape, `~0` or `~1`.
pub(crate) fn check_escapes(path: &str) -> Result<(), PointerError> {
    let mut rest = path;
    while let Some(at) = rest.find('~') {
        rest = &rest[at + 1..];
        if !rest.starts_with(['0', '1']) {
            return Err(PointerError::InvalidEscape {
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}

/// Resolves a data-binding path against the path of its scope.
///
/// An absolute `path` ignores `context`. A relative one is joined to it, with
/// trailing slashes on the context dropped. An empty path or `.` is the
/// context itself, and no context is the root.
#[must_use]
pub fn resolve_path(path: &str, context: Option<&str>) -> String {
    if path.starts_with('/') {
        return path.to_owned();
    }
    let base = context.unwrap_or("").trim_end_matches('/');
    match path {
        "" | "." if base.is_empty() => "/".to_owned(),
        "" | "." => base.to_owned(),
        _ => format!("{base}/{path}"),
    }
}

#[cfg(test)]
#[path = "pointer_tests.rs"]
mod tests;
