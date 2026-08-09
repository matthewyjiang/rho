use pretty_assertions::assert_eq;

use super::{client_info, McpRoots};

// Covers: `initialize` must identify Rho rather than the client library, must
// declare roots only when there is a workspace to serve, and must not promise a
// `roots/list_changed` notification Rho never sends.
// Owner: MCP client identity and capability declaration.
#[test]
fn client_info_identifies_rho_and_declares_roots_when_present() {
    let directory = tempfile::tempdir().unwrap();
    let with_roots = client_info(&McpRoots::for_workspace(directory.path()));

    assert_eq!(
        (
            with_roots.client_info.name.as_str(),
            with_roots.client_info.version.as_str(),
        ),
        ("rho", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        with_roots
            .capabilities
            .roots
            .and_then(|roots| roots.list_changed),
        Some(false)
    );

    let without_roots = client_info(&McpRoots::default());
    assert!(without_roots.capabilities.roots.is_none());
}
