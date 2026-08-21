use pretty_assertions::assert_eq;
use tempfile::tempdir;

use crate::tools::mcp::McpResource;

use super::{
    super::{tests::test_app, ComposerAttachment, PendingAttachmentSource},
    FilePaletteEntry,
};

/// What picking one palette row did to the composer.
#[derive(Debug, PartialEq, Eq)]
enum SelectionEffect {
    /// The message reads this, and nothing was attached.
    Reference(String),
    /// The message reads this, and one resource read is under way.
    PendingResource { text: String, name: String },
}

fn resource(uri: &str, templated: bool) -> McpResource {
    McpResource {
        server: "docs".into(),
        uri: uri.into(),
        name: "doc".into(),
        title: None,
        description: None,
        mime_type: None,
        templated,
    }
}

fn select(entry: FilePaletteEntry) -> SelectionEffect {
    let mut app = test_app();
    app.insert_pasted_input_text("look at @doc");
    app.apply_file_palette_selection(&entry).unwrap();

    match app.input_ui.attachments().as_slice() {
        [] => SelectionEffect::Reference(app.input_ui.text().to_string()),
        [ComposerAttachment::Pending {
            source: PendingAttachmentSource::McpResource,
            name,
            ..
        }] => SelectionEffect::PendingResource {
            text: app.input_ui.text().to_string(),
            name: name.clone(),
        },
        other => panic!("unexpected composer attachments: {other:?}"),
    }
}

// Covers: the palette must branch on what a row *is*. A workspace path and a URI
// template are names the message carries as text; a concrete resource is content
// and must be attached with its mention removed. Getting this wrong either sends
// the model a URI it cannot open or silently drops a file mention.
// Owner: TUI file palette selection policy.
//
// Not a PTY scenario: the resource rows need a connected MCP server, and the PTY
// harness has no MCP server fixture. The workspace-file row is also covered end
// to end by the `file_path_autocomplete` PTY scenario.
#[tokio::test]
async fn selection_inserts_names_as_text_and_attaches_content() {
    let cases = [
        (
            "a workspace file is still only a path",
            FilePaletteEntry::WorkspaceFile("src/lib.rs".into()),
            SelectionEffect::Reference("look at @src/lib.rs ".into()),
        ),
        (
            "a template cannot be read, so the user fills it in",
            FilePaletteEntry::McpResource(resource("res://users/{id}", /*templated*/ true)),
            SelectionEffect::Reference("look at @res://users/{id} ".into()),
        ),
        (
            "a concrete resource is pulled in, mention and all",
            FilePaletteEntry::McpResource(resource("res://doc", /*templated*/ false)),
            SelectionEffect::PendingResource {
                text: "look at ".into(),
                name: "res://doc".into(),
            },
        ),
    ];

    for (name, entry, expected) in cases {
        assert_eq!(select(entry), expected, "{name}");
    }
}

// Covers: expired app-owned file palette cache must rediscover workspace files.
// Owner: tui palette cache
#[test]
fn expired_app_file_cache_rediscovers_workspace_files() {
    let workspace = tempdir().unwrap();
    std::fs::write(workspace.path().join("old.rs"), "").unwrap();
    let mut app = test_app();
    app.info.runtime.cwd = workspace.path().to_path_buf();
    app.input_ui.set_text("@".to_string());
    app.input_ui.set_cursor(1);

    let first = app.file_match_list();
    assert_eq!(first.len(), 1);
    std::fs::write(workspace.path().join("new.rs"), "").unwrap();
    app.palette_caches.expire_file();
    let refreshed = app.file_match_list();
    assert_eq!(refreshed.len(), 2);
}
