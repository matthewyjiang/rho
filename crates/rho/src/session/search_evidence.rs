//! Display evidence only: snapshots, provider envelopes and token accounting
//! never enter the retrieval index. Anchors bind a JSONL position to its evidence.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct Evidence {
    pub anchor: String,
    pub role: String,
    pub text: String,
    pub omitted_blocks: usize,
}

impl Evidence {
    /// Appends preserve anchors; replacement evidence at the same position does
    /// not. Hash field boundaries explicitly, including a fixed-width count.
    fn new(position: String, role: &str, text: String, omitted_blocks: usize) -> Self {
        let mut digest = Sha256::new();
        digest.update(role.as_bytes());
        digest.update([0]);
        digest.update(text.as_bytes());
        digest.update((omitted_blocks as u64).to_le_bytes());
        Self {
            anchor: format!("{position}:{:x}", digest.finalize()),
            role: role.into(),
            text,
            omitted_blocks,
        }
    }
}

/// Extract one record without replaying model snapshots or inventing summaries.
/// Node evidence includes abandoned branches; the anchor identifies its source,
/// rather than pretending it is all part of the current conversation branch.
pub(super) fn extract(record: &Value, offset: u64) -> Vec<Evidence> {
    let messages: Vec<&Value> = match record["type"].as_str() {
        Some("message") => vec![record
            .get("display_message")
            .filter(|value| !value.is_null())
            .unwrap_or(&record["message"])],
        Some("node" | "snapshot" | "snapshot_delta") => record["display_messages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| &entry["message"])
            .collect(),
        _ => Vec::new(),
    };
    messages
        .into_iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let (kind, body) = message.as_object()?.iter().next()?;
            let anchor = format!("{offset:x}:{index}");
            if kind == "ToolResult" {
                return Some(Evidence::new(
                    anchor,
                    if body["ok"].as_bool() == Some(false) {
                        "tool_error"
                    } else {
                        "tool_result"
                    },
                    body["content"].as_str()?.to_owned(),
                    /*omitted_blocks*/ 0,
                ));
            }
            let (role, blocks) = match kind.as_str() {
                "User" => ("user", body.as_array()?),
                "Assistant" => ("assistant", body.as_array()?),
                "EnrichedAssistant" => ("assistant", body["content"].as_array()?),
                "AbortedAssistant" => ("assistant_aborted", body["content"].as_array()?),
                _ => return None,
            };
            let mut parts = Vec::new();
            let mut omitted_blocks = 0;
            for block in blocks {
                if let Some(text) = block["Text"].as_str() {
                    parts.push(text.to_owned());
                } else if let Some(call) = block.get("ToolCall") {
                    parts.push(format!(
                        "tool_call {} {}",
                        call["name"].as_str().unwrap_or("unknown"),
                        call["arguments"]
                    ));
                } else {
                    // Reasoning, image/audio bytes and provider-private blocks
                    // are not textual evidence. Report omissions on reads.
                    omitted_blocks += 1;
                }
            }
            if kind == "AbortedAssistant" {
                for call in body["tool_calls"].as_array().into_iter().flatten() {
                    parts.push(format!(
                        "partial_tool_call {} {}",
                        call["name"].as_str().unwrap_or("unknown"),
                        call["arguments"].as_str().unwrap_or("")
                    ));
                }
            }
            Some(Evidence::new(
                anchor,
                role,
                parts.join("\n"),
                omitted_blocks,
            ))
        })
        .collect()
}
