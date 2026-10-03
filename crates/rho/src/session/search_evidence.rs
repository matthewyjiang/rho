//! Display evidence only: snapshots, provider envelopes and token accounting
//! never enter the retrieval index. Anchors bind a JSONL position to its evidence.

use std::collections::HashSet;

use serde::Serialize;

use super::search::TOOL_NAME;
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
            anchor: format!("{position}:{}", hex::encode(digest.finalize())),
            role: role.into(),
            text,
            omitted_blocks,
        }
    }
}

/// Extracts one transcript's evidence, record by record in file order. The
/// sessions tool's calls and results only echo queries and other evidence, so
/// they are dropped; call ids are remembered to drop their later results.
#[derive(Default)]
pub(super) struct Extractor {
    sessions_calls: HashSet<String>,
}

impl Extractor {
    /// Extract one record without replaying model snapshots or inventing summaries.
    /// Node evidence includes abandoned branches; the anchor identifies its source,
    /// rather than pretending it is all part of the current conversation branch.
    pub(super) fn extract(&mut self, record: &Value, offset: u64) -> Vec<Evidence> {
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
                    if body["id"]
                        .as_str()
                        .is_some_and(|id| self.sessions_calls.contains(id))
                    {
                        return None;
                    }
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
                        if call["name"] == TOOL_NAME {
                            if let Some(id) = call["id"].as_str() {
                                self.sessions_calls.insert(id.to_owned());
                            }
                            continue;
                        }
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
                    for call in body["tool_calls"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|call| call["name"] != TOOL_NAME)
                    {
                        parts.push(format!(
                            "partial_tool_call {} {}",
                            call["name"].as_str().unwrap_or("unknown"),
                            call["arguments"].as_str().unwrap_or("")
                        ));
                    }
                }
                // A message holding only a `sessions` call has no evidence left.
                if parts.is_empty() && omitted_blocks == 0 {
                    return None;
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
}
