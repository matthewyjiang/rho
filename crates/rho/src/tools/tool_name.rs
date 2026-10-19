//! Provider-safe names for tools generated from external catalogs.
//!
//! Generated names are `<prefix>__<segment>__<segment>`. Providers accept only
//! `[a-zA-Z0-9_-]` and at most [`MAX_TOOL_NAME_LEN`] characters, so producers
//! build segments from these rules and cap the joined name with
//! [`cap_tool_name`]. Both steps are lossy; the registering owner must reject
//! duplicate results, as MCP registration does.

use sha2::{Digest as _, Sha256};

/// The tightest provider limit: OpenAI documents function names as
/// `[a-zA-Z0-9_-]` with a maximum length of 64, and Anthropic tool names match
/// `^[a-zA-Z0-9_-]{1,64}$`.
const MAX_TOOL_NAME_LEN: usize = 64;

/// Hex digits of SHA-256 kept as the suffix of a capped name. With 32 bits, a
/// suffix collision among GitHub's 196 over-long operations is below 1 in
/// 200,000 (196^2 / 2^33), before also requiring a shared 55-character prefix.
const HASH_SUFFIX_LEN: usize = 8;

/// Map arbitrary text to a readable name segment.
///
/// ASCII letters and digits survive. Every run of other characters, such as
/// `/`, `-`, `.`, or `_`, becomes one `_`, and edge `_` are trimmed.
/// The result never contains the `__` separator, so a joined name keeps its
/// segment boundaries. Text with no usable characters becomes a hash of itself.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "OpenAPI tool names adopt the sanitizer next")
)]
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
/// Longer names keep a readable prefix and end in `_` plus a hash of the whole
/// name, so long names that share a prefix stay distinct.
pub(crate) fn cap_tool_name(name: String) -> String {
    if name.len() <= MAX_TOOL_NAME_LEN {
        return name;
    }
    let mut end = MAX_TOOL_NAME_LEN - HASH_SUFFIX_LEN - 1;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    // A trailing `_` would join the suffix into a `__` segment separator.
    let prefix = name[..end].trim_end_matches('_');
    format!("{prefix}_{}", short_hash(&name))
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut hex = String::with_capacity(HASH_SUFFIX_LEN);
    for byte in &digest[..HASH_SUFFIX_LEN / 2] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
#[path = "tool_name_tests.rs"]
mod tests;
