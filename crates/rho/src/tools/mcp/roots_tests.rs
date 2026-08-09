// Root field access carries rmcp's SEP-2577 deprecation marker; Rho
// implements the current wire protocol.
#![expect(deprecated)]

use pretty_assertions::assert_eq;

use super::McpRoots;

// Covers: an advertised root must reach servers as a named `file://` URL, and a
// path that cannot become one must be advertised as nothing rather than as a
// value servers cannot parse.
// Owner: MCP roots advertisement.
#[test]
fn workspace_roots_become_named_file_urls() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("project");
    std::fs::create_dir(&workspace).unwrap();

    let roots = McpRoots::for_workspace(&workspace);
    let protocol = roots.to_protocol();

    assert_eq!(protocol.len(), 1);
    assert_eq!(protocol[0].name.as_deref(), Some("project"));
    assert!(protocol[0].uri.starts_with("file://"));
    assert!(protocol[0].uri.ends_with('/'));

    let relative = McpRoots::for_workspace(std::path::Path::new("relative/path"));
    assert!(relative.is_empty());
    assert!(relative.to_protocol().is_empty());
}
