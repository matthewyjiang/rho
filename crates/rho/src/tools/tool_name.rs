//! Provider-safe names for tools generated from external catalogs.
//!
//! Providers accept only `[a-zA-Z0-9_-]` and at most [`MAX_TOOL_NAME_LEN`]
//! characters, so every generated name goes through [`cap_tool_name`]. A capped
//! name keeps a readable prefix and ends in [`CAP_MARKER`] plus a hash of the
//! whole name. Producers never emit `___` before a hex digit, so that suffix
//! marks a capped name unambiguously ([`is_capped_tool_name`]):
//!
//! - OpenAPI names are `<prefix>__<segment>__<segment>`, and
//!   [`sanitize_segment`] never starts or ends a segment with `_`.
//! - MCP names are `mcp__<server>__<tool>` (`tools::mcp::exported_name`), whose
//!   components start with `_` only as the `_rho_` escape.
//!
//! Capping is lossy, so the registering owner must reject duplicate results,
//! and anything that needs a capped tool's identity must look it up rather
//! than parse the name.

use sha2::{Digest as _, Sha256};

/// The tightest provider limit: OpenAI documents function names as
/// `[a-zA-Z0-9_-]` with a maximum length of 64, and Anthropic tool names match
/// `^[a-zA-Z0-9_-]{1,64}$`.
const MAX_TOOL_NAME_LEN: usize = 64;

/// Hex digits of SHA-256 kept as the suffix of a capped name. Only capped names
/// sharing a 53-character prefix can collide; for `n` of them the chance is
/// about `n^2 / 2^33`, under 1 in 200,000 even for `n = 196`. Registration
/// still rejects a collision rather than trusting the odds.
const HASH_SUFFIX_LEN: usize = 8;

/// Joins a capped name's readable prefix to its hash.
const CAP_MARKER: &str = "___";

/// Map arbitrary text to a readable name segment.
///
/// ASCII letters and digits survive. Every run of other characters, such as
/// `/`, `-`, `.`, or `_`, becomes one `_`, and edge `_` are trimmed.
/// The result never contains the `__` separator, so a joined name keeps its
/// segment boundaries. Text with no usable characters becomes a hash of itself.
pub(crate) fn sanitize_segment(raw: &str) -> String {
    let mut segment = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            segment.push(character);
        } else if !segment.is_empty() && !segment.ends_with('_') {
            segment.push('_');
        }
    }
    if segment.ends_with('_') {
        segment.pop();
    }
    if segment.is_empty() {
        return short_hash(raw);
    }
    segment
}

/// Cap a name at [`MAX_TOOL_NAME_LEN`]. Names within the limit are unchanged.
/// Longer names keep a readable prefix and end in [`CAP_MARKER`] plus a hash
/// of the whole name, so long names that share a prefix stay distinct.
pub(crate) fn cap_tool_name(name: String) -> String {
    if name.len() <= MAX_TOOL_NAME_LEN {
        return name;
    }
    let mut end = MAX_TOOL_NAME_LEN - HASH_SUFFIX_LEN - CAP_MARKER.len();
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    // Trailing `_` would merge into the marker and blur where it starts.
    let prefix = name[..end].trim_end_matches('_');
    format!("{prefix}{CAP_MARKER}{}", short_hash(&name))
}

/// Whether [`cap_tool_name`] shortened `name`, which then no longer encodes
/// its tool's identity.
pub(crate) fn is_capped_tool_name(name: &str) -> bool {
    name.rsplit_once(CAP_MARKER).is_some_and(|(_, hash)| {
        hash.len() == HASH_SUFFIX_LEN
            && hash
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

fn short_hash(value: &str) -> String {
    hex::encode(&Sha256::digest(value.as_bytes())[..HASH_SUFFIX_LEN / 2])
}

#[cfg(test)]
#[path = "tool_name_tests.rs"]
mod tests;
