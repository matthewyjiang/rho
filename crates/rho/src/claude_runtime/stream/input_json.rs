//! Assembly of streamed `input_json_delta` fragments into tool input.
//!
//! Parsing the whole buffer on every fragment is quadratic in the payload
//! size. A byte scanner carried across fragments tracks just enough JSON
//! structure (nesting depth and string state) to know when a top-level field
//! closes, so large bodies are re-parsed only at field boundaries.

use serde_json::Value;

use rho_sdk::floor_char_boundary;

use crate::cli_runtime::stream_effect::MAX_TOOL_PAYLOAD_CHARS;

/// Raw `input_json_delta` assembly budget. Larger than the presentation cap
/// so a complete oversized object can be parsed, then reduced by the card
/// input bounding.
pub(super) const MAX_INPUT_JSON_CHARS: usize = MAX_TOOL_PAYLOAD_CHARS.saturating_mul(16);

/// Buffers at or under this size re-parse on every fragment, so early fields
/// such as `command` or `file_path` show progressively on the running card.
/// Past it, parses wait for a top-level field boundary: bounded card input is
/// capped at the same size, so re-parsing a growing body changes nothing the
/// card can show.
const EAGER_PARSE_CHARS: usize = MAX_TOOL_PAYLOAD_CHARS;

/// Concatenated fragments plus the scanner state at the end of the buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct StreamedInputJson {
    raw: String,
    scan: JsonScan,
}

impl StreamedInputJson {
    /// Append a fragment (bounded by [`MAX_INPUT_JSON_CHARS`]). Returns the
    /// parsed object when this fragment made a re-parse worthwhile and the
    /// buffer (possibly with a small closer) parses.
    pub(super) fn push(&mut self, fragment: &str) -> Option<Value> {
        if fragment.is_empty() {
            return None;
        }
        let room = MAX_INPUT_JSON_CHARS.saturating_sub(self.raw.len());
        let end = floor_char_boundary(fragment, room);
        if end == 0 {
            return None;
        }
        let appended = &fragment[..end];
        let truncated = end < fragment.len();
        self.raw.push_str(appended);
        let crossed_boundary = self.scan.advance(appended);
        let due = crossed_boundary || truncated || self.raw.len() <= EAGER_PARSE_CHARS;
        if !due {
            return None;
        }
        parse_assembled_input(&self.raw)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.raw.len()
    }
}

/// JSON lexical state after the bytes scanned so far. Structural bytes are
/// ASCII, so scanning UTF-8 bytewise never misreads a multibyte character.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct JsonScan {
    depth: usize,
    in_string: bool,
    escaped: bool,
}

impl JsonScan {
    /// Scan `text`, returning true when a top-level field closed: a `,`
    /// between top-level members or the brace that closes the object.
    fn advance(&mut self, text: &str) -> bool {
        let mut crossed = false;
        for byte in text.bytes() {
            if self.in_string {
                match byte {
                    _ if self.escaped => self.escaped = false,
                    b'\\' => self.escaped = true,
                    b'"' => self.in_string = false,
                    _ => {}
                }
                continue;
            }
            match byte {
                b'"' => self.in_string = true,
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => {
                    self.depth = self.depth.saturating_sub(1);
                    crossed |= self.depth == 0;
                }
                b',' => crossed |= self.depth == 1,
                _ => {}
            }
        }
        crossed
    }
}

/// Parse assembled `input_json_delta` text. Truncated objects keep leading
/// keys when a small closer produces valid JSON.
fn parse_assembled_input(raw: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(raw) {
        return Some(value);
    }
    for suffix in ["}", "\"}"] {
        if let Ok(value) = serde_json::from_str::<Value>(&format!("{raw}{suffix}")) {
            if value.is_object() {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "input_json_tests.rs"]
mod tests;
