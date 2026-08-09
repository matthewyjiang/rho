//! Turning an MCP `tools/call` result into what the model reads and the tool
//! card shows.
//!
//! The result is a list of typed content blocks plus optional structured data.
//! Serializing the whole envelope to JSON, as Rho used to, spent the model's
//! context on protocol scaffolding and buried a returned image in a base64
//! string. Each block is rendered for what it is instead: text stays text,
//! binary becomes a card asset with a short descriptor, and structured content
//! is presented once rather than twice.

use rho_sdk::tool::{ToolAsset, ToolError, ToolErrorKind};
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};

use base64::Engine;

/// What one MCP result becomes inside Rho.
#[derive(Debug, Default, PartialEq)]
pub(super) struct RenderedResult {
    /// The text handed to the model.
    pub(super) text: String,
    /// Binary content the tool card can render, in the order it arrived.
    pub(super) assets: Vec<ToolAsset>,
}

/// What the tool's own declaration says its result must contain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ResultExpectation {
    /// The server declared an `outputSchema`, so the spec requires it to return
    /// `structuredContent` on success.
    pub(super) structured_content: bool,
}

/// Render a successful or failed call. An MCP error result becomes a tool
/// failure carrying the same rendered text, so the model sees what went wrong
/// rather than a JSON envelope.
pub(super) fn render(
    result: &CallToolResult,
    expectation: ResultExpectation,
    max_output_bytes: usize,
) -> Result<RenderedResult, ToolError> {
    let failed = result.is_error.unwrap_or(false);
    let mut rendered = RenderedResult::default();
    let mut sections = Vec::new();
    for block in &result.content {
        if let Some(section) = render_block(block, &mut rendered.assets) {
            sections.push(section);
        }
    }

    if let Some(structured) = &result.structured_content {
        // Servers are asked to mirror structured content as text for clients
        // that predate it. Keeping both would spend the context twice.
        sections.retain(|section| !mirrors(section, structured));
        sections.push(
            serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string()),
        );
    } else if expectation.structured_content && !failed {
        return Err(ToolError::new(
            ToolErrorKind::Execution,
            "MCP server declared an output schema but returned no structured content",
        ));
    }

    if sections.is_empty() {
        sections.push("The MCP server returned no content.".into());
    }
    rendered.text = rho_tools::tool::truncate(sections.join("\n\n"), max_output_bytes);

    if failed {
        return Err(ToolError::new(ToolErrorKind::Execution, rendered.text));
    }
    Ok(rendered)
}

/// Whether a text section is just the structured content written out, in either
/// the compact or the pretty form servers commonly use.
fn mirrors(section: &str, structured: &serde_json::Value) -> bool {
    let trimmed = section.trim();
    serde_json::from_str::<serde_json::Value>(trimmed).is_ok_and(|parsed| &parsed == structured)
}

/// Render one block, pushing any renderable binary onto `assets`. Returns the
/// text this block contributes, if any.
fn render_block(block: &ContentBlock, assets: &mut Vec<ToolAsset>) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(text.text.clone()),
        ContentBlock::Image(image) => Some(binary_section(
            "image",
            &image.mime_type,
            &image.data,
            assets,
        )),
        ContentBlock::Audio(audio) => Some(binary_section(
            "audio",
            &audio.mime_type,
            &audio.data,
            assets,
        )),
        ContentBlock::Resource(embedded) => Some(match &embedded.resource {
            ResourceContents::TextResourceContents { uri, text, .. } => {
                format!("[resource {uri}]\n{text}")
            }
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => {
                let media_type = mime_type.as_deref().unwrap_or("application/octet-stream");
                let descriptor = binary_section("resource", media_type, blob, assets);
                format!("[resource {uri}] {descriptor}")
            }
            // `ResourceContents` is non-exhaustive: a kind from a newer spec
            // revision is named rather than dropped.
            _ => "[resource of a kind Rho does not render]".into(),
        }),
        ContentBlock::ResourceLink(link) => {
            let mut line = format!("[resource link {} \"{}\"]", link.uri, link.name);
            if let Some(description) = &link.description {
                line.push_str(&format!("\n{description}"));
            }
            Some(line)
        }
        // `ContentBlock` is non-exhaustive: a block from a newer spec revision
        // is named rather than dropped, so the model knows something arrived.
        _ => Some("[content block of a kind Rho does not render]".into()),
    }
}

/// Describe binary content and, when Rho can render it, keep the bytes as a
/// card asset. The base64 payload never reaches the model: it would be a large
/// unreadable string that no model can act on.
fn binary_section(
    label: &str,
    media_type: &str,
    encoded: &str,
    assets: &mut Vec<ToolAsset>,
) -> String {
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return format!("[{label} {media_type}, not valid base64]");
    };
    let size = bytes.len();
    if media_type.starts_with("image/") {
        assets.push(ToolAsset::new(media_type.to_string(), bytes));
    }
    format!("[{label} {media_type}, {}]", byte_size(size))
}

fn byte_size(bytes: usize) -> String {
    const UNIT: f64 = 1024.0;
    let bytes = bytes as f64;
    if bytes < UNIT {
        return format!("{bytes:.0} B");
    }
    if bytes < UNIT * UNIT {
        return format!("{:.1} KB", bytes / UNIT);
    }
    format!("{:.1} MB", bytes / (UNIT * UNIT))
}

#[cfg(test)]
#[path = "result_tests.rs"]
mod tests;
