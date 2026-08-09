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

/// Encoded image bytes one MCP result may retain for card display.
///
/// The interactive card shows one image. The TUI decode path allows up to
/// 8 MiB of decoded pixels; encoded sources are smaller, so 4 MiB is a
/// generous tripwire for one screenshot while still bounding memory an
/// untrusted server can force Rho to hold on a single tool result.
pub(super) const MAX_RETAINED_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// Serialized JSON Schema bytes Rho will compile for `outputSchema` validation.
const MAX_OUTPUT_SCHEMA_BYTES: usize = 64 * 1024;

/// Serialized structured-content bytes Rho will walk during schema validation.
const MAX_STRUCTURED_CONTENT_BYTES: usize = 256 * 1024;

/// Maximum JSON nodes (objects, arrays, and leaves) in a declared output schema.
const MAX_OUTPUT_SCHEMA_NODES: usize = 2_048;

/// Maximum JSON nodes in structured content accepted for schema validation.
const MAX_STRUCTURED_CONTENT_NODES: usize = 8_192;

/// What one MCP result becomes inside Rho.
#[derive(Debug, Default, PartialEq)]
pub(super) struct RenderedResult {
    /// The text handed to the model.
    pub(super) text: String,
    /// Binary content the tool card can render, in the order it arrived.
    pub(super) assets: Vec<ToolAsset>,
}

/// What the tool's own declaration says its result must contain.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ResultExpectation {
    /// Declared `outputSchema`, when present. A successful result must include
    /// `structuredContent` that validates against it.
    pub(super) output_schema: Option<serde_json::Value>,
}

/// Tracks which image bytes this result still keeps for the card.
#[derive(Default)]
struct AssetBudget {
    retained_bytes: usize,
    retained_images: usize,
}

/// Render a successful or failed call. An MCP error result becomes a tool
/// failure carrying the same rendered text, so the model sees what went wrong
/// rather than a JSON envelope.
pub(super) fn render(
    result: &CallToolResult,
    expectation: &ResultExpectation,
    max_output_bytes: usize,
) -> Result<RenderedResult, ToolError> {
    let failed = result.is_error.unwrap_or(false);
    let mut rendered = RenderedResult::default();
    let mut budget = AssetBudget::default();
    let mut sections = Vec::new();
    for block in &result.content {
        if let Some(section) = render_block(block, &mut rendered.assets, &mut budget) {
            sections.push(section);
        }
    }

    if let Some(structured) = &result.structured_content {
        if !failed {
            if let Some(schema) = &expectation.output_schema {
                validate_structured_content(schema, structured)?;
            }
        }
        // Servers are asked to mirror structured content as text for clients
        // that predate it. Keeping both would spend the context twice.
        sections.retain(|section| !mirrors(section, structured));
        sections.push(
            serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string()),
        );
    } else if expectation.output_schema.is_some() && !failed {
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

/// Render the messages one `prompts/get` returned into composer text.
///
/// A prompt is a conversation the server suggests. Rho seeds it as one user
/// message, because the composer submits one turn, so each message is labelled
/// with the role the server assigned it. Binary blocks become descriptors only;
/// a prompt is text the user is about to send, not a tool card.
pub(super) fn render_prompt_messages(
    messages: &[rmcp::model::PromptMessage],
    max_output_bytes: usize,
) -> String {
    let mut assets = Vec::new();
    let mut budget = AssetBudget::default();
    let sections = messages
        .iter()
        .filter_map(|message| {
            let body = render_block(&message.content, &mut assets, &mut budget)?;
            Some(match message.role {
                rmcp::model::Role::User => body,
                // Anything the server puts in the assistant's mouth is labelled,
                // so the model reads it as prior context and not as a request.
                _ => format!("[assistant]\n{body}"),
            })
        })
        .collect::<Vec<_>>();
    rho_tools::tool::truncate(sections.join("\n\n"), max_output_bytes)
}

/// Fail when structured content does not match the tool's declared schema.
fn validate_structured_content(
    schema: &serde_json::Value,
    structured: &serde_json::Value,
) -> Result<(), ToolError> {
    // Bound work before compiling or walking anything server-controlled. The
    // schema and instance both come from an untrusted MCP server.
    ensure_json_budget(schema, MAX_OUTPUT_SCHEMA_BYTES, "output schema")?;
    ensure_json_budget(
        structured,
        MAX_STRUCTURED_CONTENT_BYTES,
        "structured content",
    )?;
    ensure_node_budget(schema, MAX_OUTPUT_SCHEMA_NODES, "output schema")?;
    ensure_node_budget(
        structured,
        MAX_STRUCTURED_CONTENT_NODES,
        "structured content",
    )?;

    // No remote or file $ref resolution: default-features disable the retrieve
    // backends. Prefer the linear `regex` pattern engine so a hostile pattern
    // cannot spend exponential backtracking time on every tools/call.
    let validator = jsonschema::options()
        .with_pattern_options(jsonschema::PatternOptions::regex())
        .build(schema)
        .map_err(|error| {
            ToolError::new(
                ToolErrorKind::Execution,
                format!("MCP server declared an invalid output schema: {error}"),
            )
        })?;
    if let Err(error) = validator.validate(structured) {
        return Err(ToolError::new(
            ToolErrorKind::Execution,
            format!("MCP structured content failed the declared output schema: {error}"),
        ));
    }
    Ok(())
}

fn ensure_json_budget(
    value: &serde_json::Value,
    max_bytes: usize,
    label: &str,
) -> Result<(), ToolError> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        ToolError::new(
            ToolErrorKind::Execution,
            format!("MCP server returned {label} that could not be measured: {error}"),
        )
    })?;
    if encoded.len() > max_bytes {
        return Err(ToolError::new(
            ToolErrorKind::Execution,
            format!(
                "MCP server {label} exceeds the {} validation budget",
                byte_size(max_bytes)
            ),
        ));
    }
    Ok(())
}

fn ensure_node_budget(
    value: &serde_json::Value,
    max_nodes: usize,
    label: &str,
) -> Result<(), ToolError> {
    let mut count = 0usize;
    let mut stack = vec![value];
    while let Some(next) = stack.pop() {
        count = count.saturating_add(1);
        if count > max_nodes {
            return Err(ToolError::new(
                ToolErrorKind::Execution,
                format!("MCP server {label} exceeds the {max_nodes}-node validation budget"),
            ));
        }
        match next {
            serde_json::Value::Array(items) => stack.extend(items.iter()),
            serde_json::Value::Object(map) => stack.extend(map.values()),
            _ => {}
        }
    }
    Ok(())
}

/// Whether a text section is just the structured content written out, in either
/// the compact or the pretty form servers commonly use.
fn mirrors(section: &str, structured: &serde_json::Value) -> bool {
    let trimmed = section.trim();
    serde_json::from_str::<serde_json::Value>(trimmed).is_ok_and(|parsed| &parsed == structured)
}

/// Render one block, pushing any renderable binary onto `assets`. Returns the
/// text this block contributes, if any.
fn render_block(
    block: &ContentBlock,
    assets: &mut Vec<ToolAsset>,
    budget: &mut AssetBudget,
) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(text.text.clone()),
        ContentBlock::Image(image) => Some(binary_section(
            "image",
            &image.mime_type,
            &image.data,
            assets,
            budget,
        )),
        ContentBlock::Audio(audio) => Some(binary_section(
            "audio",
            &audio.mime_type,
            &audio.data,
            assets,
            budget,
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
                let descriptor = binary_section("resource", media_type, blob, assets, budget);
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
///
/// The card shows one image, so only the first image that fits the retained
/// budget is kept. Later or oversized images stay as descriptors only.
fn binary_section(
    label: &str,
    media_type: &str,
    encoded: &str,
    assets: &mut Vec<ToolAsset>,
    budget: &mut AssetBudget,
) -> String {
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return format!("[{label} {media_type}, not valid base64]");
    };
    let size = bytes.len();
    let descriptor = format!("[{label} {media_type}, {}]", byte_size(size));
    if !media_type.starts_with("image/") {
        return descriptor;
    }
    if budget.retained_images > 0 {
        return format!("{descriptor} [not shown: card keeps only the first image]");
    }
    if size > MAX_RETAINED_IMAGE_BYTES {
        return format!(
            "{descriptor} [not retained: exceeds {} asset budget]",
            byte_size(MAX_RETAINED_IMAGE_BYTES)
        );
    }
    budget.retained_images += 1;
    budget.retained_bytes = budget.retained_bytes.saturating_add(size);
    assets.push(ToolAsset::new(media_type.to_string(), bytes));
    descriptor
}

/// The one line Rho uses to stand in for binary it cannot show as text.
///
/// Hosts that keep the bytes elsewhere, such as the composer keeping an image
/// attachment, still describe them this way so a person sees the same wording
/// wherever the content came from. `decoded_size` is `None` when the payload was
/// not valid base64.
pub(crate) fn binary_descriptor(
    label: &str,
    media_type: &str,
    decoded_size: Option<usize>,
) -> String {
    match decoded_size {
        Some(size) => format!("[{label} {media_type}, {}]", byte_size(size)),
        None => format!("[{label} {media_type}, not valid base64]"),
    }
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
