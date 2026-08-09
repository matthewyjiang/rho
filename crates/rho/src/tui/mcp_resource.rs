//! Pulling an MCP resource into the message being written.
//!
//! Picking a resource from the `@` palette is not a reference, it is content:
//! the server holds the bytes and the message needs them. So the mention token
//! is removed and the resource arrives as a composer attachment, the same way a
//! pasted file does, which is also what gives it submit-gating, backspace
//! cancel, and teardown on `/clear` without any of its own bookkeeping.
//!
//! `resources/read` is a round-trip, so it runs as a task and the composer shows
//! a pending entry meanwhile.

use rho_providers::model::ImageContent;

use crate::tools::mcp::{McpCatalog, McpCatalogError, McpResource, McpResourceContent};

use super::{
    media_attach::{MediaAttachOutcome, MediaAttachTask},
    App, ChatMedia, ChatTextDocument, ComposerMode, MediaAttachId, PendingAttachmentSource,
};

impl App {
    /// Start reading one concrete resource into an attachment.
    pub(super) fn start_mcp_resource_attach(
        &mut self,
        resource: &McpResource,
    ) -> anyhow::Result<()> {
        // The same two guards a pasted file gets, for the same reason: an
        // attachment belongs to the message being written, and there is no
        // message being written in either of these states.
        if self.is_ui_busy() {
            self.notify_status("resources cannot be attached while a model turn is running");
            return Ok(());
        }
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            self.notify_status("resources can only be attached from the message box");
            return Ok(());
        }

        // Resource text is capped by the same limit as tool output, so one very
        // large resource cannot swamp the turn it joins.
        let max_output_bytes = self
            .info
            .services
            .config_repository
            .load()?
            .max_output_bytes;

        // The mention itself goes away: the message carries the content, so a
        // leftover `@uri` would only read as a path the model should open.
        self.clear_active_file_mention();

        let id = MediaAttachId::new();
        let catalog = self.mcp_catalog.clone();
        let server = resource.server.clone();
        let uri = resource.uri.clone();
        let name = resource.uri.clone();
        let task = tokio::spawn(async move {
            read_resource_attachment(catalog, server, uri, max_output_bytes).await
        });
        self.media_attach_tasks.push(MediaAttachTask { id, task });
        self.input_ui.push_pending_attachment(
            id,
            PendingAttachmentSource::McpResource,
            name.clone(),
        );
        self.notify_status(format!("fetching {name}"));
        Ok(())
    }
}

/// Read one resource and turn its bodies into a single attachment.
async fn read_resource_attachment(
    catalog: McpCatalog,
    server: String,
    uri: String,
    max_output_bytes: usize,
) -> MediaAttachOutcome {
    match catalog.read_resource(&server, &uri).await {
        Ok(contents) => resource_attachment(&uri, &contents, max_output_bytes),
        Err(error) => resource_failure(error),
    }
}

fn resource_failure(error: McpCatalogError) -> MediaAttachOutcome {
    MediaAttachOutcome::Failed {
        kind: "resource read",
        message: error.to_string(),
    }
}

/// Choose the one attachment a read produced.
///
/// A read returns a list, but the composer attaches one thing per pick. A lone
/// image becomes an image so the model can actually look at it; everything else
/// becomes one text document, so a multi-part resource arrives whole instead of
/// silently losing its other parts.
pub(super) fn resource_attachment(
    uri: &str,
    contents: &[McpResourceContent],
    max_output_bytes: usize,
) -> MediaAttachOutcome {
    if let [McpResourceContent::Blob {
        mime_type: Some(mime_type),
        blob,
        ..
    }] = contents
    {
        if mime_type.starts_with("image/") {
            // MCP blobs are already base64, which is the encoding a provider
            // wants, so the bytes are handed over untouched.
            return MediaAttachOutcome::Ready(ChatMedia::Image(ImageContent {
                data: blob.clone(),
                mime_type: mime_type.clone(),
            }));
        }
    }

    let body = contents
        .iter()
        .map(resource_body_text)
        .collect::<Vec<_>>()
        .join("\n\n");
    let truncated_body = rho_tools::tool::truncate(body.clone(), max_output_bytes);
    MediaAttachOutcome::Ready(ChatMedia::TextDocument(ChatTextDocument {
        name: uri.to_string(),
        mime: resource_mime(contents),
        truncated: truncated_body.len() != body.len(),
        body: truncated_body,
        warnings: Vec::new(),
    }))
}

/// Text stands in for itself; binary stands in for the descriptor Rho uses for
/// binary everywhere else, so a person reads the same wording whether the bytes
/// came back from a tool call or from a picked resource.
fn resource_body_text(content: &McpResourceContent) -> String {
    match content {
        McpResourceContent::Text { text, .. } => text.clone(),
        McpResourceContent::Blob {
            mime_type, blob, ..
        } => crate::tools::mcp::result::binary_descriptor(
            "resource",
            mime_type.as_deref().unwrap_or(DEFAULT_BLOB_MIME),
            decoded_size(blob),
        ),
        McpResourceContent::Unsupported => "[resource of a kind Rho does not render]".to_string(),
    }
}

/// The media type the whole attachment is labelled with. A read that returned
/// one body is described by that body; a mixed read has no single type.
fn resource_mime(contents: &[McpResourceContent]) -> String {
    match contents {
        [McpResourceContent::Text {
            mime_type: Some(mime_type),
            ..
        }]
        | [McpResourceContent::Blob {
            mime_type: Some(mime_type),
            ..
        }] => mime_type.clone(),
        [McpResourceContent::Text { .. }] => "text/plain".to_string(),
        _ => DEFAULT_BLOB_MIME.to_string(),
    }
}

const DEFAULT_BLOB_MIME: &str = "application/octet-stream";

fn decoded_size(blob: &str) -> Option<usize> {
    use base64::Engine;

    base64::engine::general_purpose::STANDARD
        .decode(blob)
        .ok()
        .map(|bytes| bytes.len())
}

#[cfg(test)]
#[path = "mcp_resource_tests.rs"]
mod tests;
