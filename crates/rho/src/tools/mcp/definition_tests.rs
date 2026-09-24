use pretty_assertions::assert_eq;
use rho_sdk::tool::OperationKind;
use rmcp::model::{Tool as RemoteTool, ToolAnnotations};

use super::{McpToolDefinition, McpTransport};

fn remote(annotations: Option<ToolAnnotations>, output_schema: bool) -> RemoteTool {
    let schema = serde_json::from_value(serde_json::json!({"type": "object"})).unwrap();
    let mut tool = RemoteTool::new("search", "find things", std::sync::Arc::new(schema));
    tool.annotations = annotations;
    if output_schema {
        tool.output_schema = Some(std::sync::Arc::new(
            serde_json::from_value(serde_json::json!({"type": "object"})).unwrap(),
        ));
    }
    tool
}

fn annotations(read_only: bool, destructive: bool, open_world: bool) -> ToolAnnotations {
    let mut annotations = ToolAnnotations::default();
    annotations.title = Some("Search".into());
    annotations.read_only_hint = Some(read_only);
    annotations.destructive_hint = Some(destructive);
    annotations.open_world_hint = Some(open_world);
    annotations
}

fn stdio() -> McpTransport {
    McpTransport::Stdio {
        command: "search-mcp".into(),
        args: vec!["--stdio".into()],
        cwd: None,
        env: Default::default(),
        env_from_env: Default::default(),
    }
}

// Covers: a server's title and behavior hints must reach the model's tool
// description and the card, an output schema must set the structured-content
// contract, and an unannotated tool must gain neither.
// Owner: MCP tool declaration mapping.
#[test]
fn annotations_and_output_schema_shape_the_exported_tool() {
    // One description line, plus one line per server behavior hint.
    let hint_lines = |description: &str| description.lines().count() - 1;

    let plain = McpToolDefinition::from_remote("docs", "search", &remote(None, false));
    assert!(plain.spec.description.contains("`docs`"));
    assert!(plain.spec.description.contains("find things"));
    assert_eq!(hint_lines(&plain.spec.description), 0);
    assert_eq!(plain.expectation.output_schema, None);

    let annotated = McpToolDefinition::from_remote(
        "docs",
        "search",
        &remote(Some(annotations(true, false, true)), true),
    );
    assert!(annotated.spec.description.contains("Search"));
    assert!(annotated.spec.description.contains("find things"));
    assert_eq!(hint_lines(&annotated.spec.description), 2);
    assert_eq!(
        annotated.expectation.output_schema,
        Some(serde_json::json!({"type": "object"}))
    );

    let destructive = McpToolDefinition::from_remote(
        "docs",
        "search",
        &remote(Some(annotations(false, true, false)), false),
    );
    assert_eq!(hint_lines(&destructive.spec.description), 1);
}

// Covers: a read-only hint must change only how a call is presented. It must
// never change what the call is permitted to do, because the hint comes from a
// server Rho does not control.
// Owner: MCP tool presentation.
#[test]
fn read_only_hint_changes_presentation_only() {
    let read_only = McpToolDefinition::from_remote(
        "docs",
        "search",
        &remote(Some(annotations(true, false, false)), false),
    );
    let writing = McpToolDefinition::from_remote("docs", "search", &remote(None, false));

    assert_eq!(
        (
            read_only.presentation.metadata(&stdio()).operation_kind(),
            writing.presentation.metadata(&stdio()).operation_kind(),
        ),
        (Some(&OperationKind::Read), Some(&OperationKind::Execute))
    );
    assert_eq!(
        (
            read_only
                .presentation
                .metadata(&stdio())
                .presentation_notices()
                .len(),
            writing
                .presentation
                .metadata(&stdio())
                .presentation_notices()
                .len(),
        ),
        (1, 0)
    );
}
