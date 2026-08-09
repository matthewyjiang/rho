//! What Rho derives from one remote tool's declaration.
//!
//! A server describes a tool with a name, description, input schema, an
//! optional output schema, and optional annotations. Rho turns that into the
//! spec the model sees, the result contract a call is checked against, and the
//! presentation facts a tool card shows.
//!
//! Annotations are hints from a server Rho does not control, so they never
//! relax a permission or an approval. They change what the user is told and
//! what the model is told, and nothing else.

use rho_sdk::{
    model::ToolSpec,
    tool::{OperationKind, ToolMetadata},
};
use rmcp::model::{Tool as RemoteTool, ToolAnnotations};

use super::{config::McpTransport, result::ResultExpectation, tool::namespaced_tool_name};

/// Everything one exported MCP tool derives from its current declaration.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct McpToolDefinition {
    pub(super) spec: ToolSpec,
    pub(super) expectation: ResultExpectation,
    pub(super) presentation: McpToolPresentation,
}

/// Card-facing facts a server's annotations imply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct McpToolPresentation {
    /// The server says the tool does not change anything.
    read_only: bool,
    /// Server-supplied cautions, shown as-is and attributed to the server.
    notices: Vec<String>,
}

impl McpToolDefinition {
    pub(super) fn from_remote(identity: &str, remote_name: &str, remote: &RemoteTool) -> Self {
        let annotations = remote.annotations.as_ref();
        let description = remote
            .description
            .as_deref()
            .unwrap_or("No description supplied by the MCP server");
        let title = annotations
            .and_then(|annotations| annotations.title.as_deref())
            .or(remote.title.as_deref());
        let mut description = match title {
            Some(title) => format!("MCP server `{identity}`, {title}: {description}"),
            None => format!("MCP server `{identity}`: {description}"),
        };
        for hint in behavior_hints(annotations) {
            description.push_str(&format!("\nServer hint: {hint}."));
        }
        Self {
            spec: ToolSpec {
                name: namespaced_tool_name(identity, remote_name),
                description,
                input_schema: serde_json::Value::Object((*remote.input_schema).clone()),
            },
            expectation: ResultExpectation {
                // The spec requires structured content whenever the tool
                // declares a schema for it.
                structured_content: remote.output_schema.is_some(),
            },
            presentation: McpToolPresentation {
                read_only: annotations
                    .is_some_and(|annotations| annotations.read_only_hint.unwrap_or(false)),
                notices: behavior_hints(annotations)
                    .into_iter()
                    .map(|hint| format!("Server hint: {hint}"))
                    .collect(),
            },
        }
    }
}

impl McpToolPresentation {
    /// Build the metadata for one call.
    ///
    /// The transport decides the default operation because that is what the
    /// host actually does. A read-only hint narrows the icon to a read, which
    /// is a display choice; it never narrows what the call is allowed to do.
    pub(super) fn metadata(&self, transport: &McpTransport) -> ToolMetadata {
        let mut metadata = match transport {
            McpTransport::Stdio { command, args, .. } => ToolMetadata::new()
                .operation(if self.read_only {
                    OperationKind::Read
                } else {
                    OperationKind::Execute
                })
                .command_summary(format!("{command} ({} arguments)", args.len())),
            McpTransport::StreamableHttp { url, .. } => ToolMetadata::new()
                .operation(if self.read_only {
                    OperationKind::Read
                } else {
                    OperationKind::Network
                })
                .url(url.clone()),
        };
        for notice in &self.notices {
            metadata = metadata.presentation_notice(notice.clone());
        }
        metadata
    }
}

/// The annotation hints worth repeating. Absent hints say nothing, and the
/// defaults in the spec are already what a reader assumes, so only the
/// meaningful settings are reported.
fn behavior_hints(annotations: Option<&ToolAnnotations>) -> Vec<&'static str> {
    let Some(annotations) = annotations else {
        return Vec::new();
    };
    let read_only = annotations.read_only_hint.unwrap_or(false);
    let mut hints = Vec::new();
    if read_only {
        hints.push("this tool only reads");
    } else if annotations.destructive_hint.unwrap_or(false) {
        hints.push("this tool may make destructive changes");
    }
    if annotations.open_world_hint.unwrap_or(false) {
        hints.push("this tool reaches systems outside this machine");
    }
    hints
}

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;
