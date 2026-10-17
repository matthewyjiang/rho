use std::{ffi::OsStr, path::PathBuf};

use pretty_assertions::assert_eq;

use super::AgentFiles;

// Covers: the workflow catalog reads only the plugin files discovery kept, so
// a symlinked file discovery reported is skipped instead of failing every
// workflow, while user agent directories read every Markdown file.
// Owner: workflow dynamic capability adapter.
#[test]
fn agent_files_read_markdown_and_only_discovered_plugin_files() {
    let kept = [PathBuf::from(
        "/plugin/io.github.matthewyjiang.rho/agents/auditor.md",
    )];
    let names = ["auditor.md", "linked.md", "notes.txt"];
    let reads = |files: AgentFiles<'_>| {
        names
            .into_iter()
            .filter(|name| files.reads(OsStr::new(name)))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        (
            reads(AgentFiles::AllMarkdown),
            reads(AgentFiles::Discovered(&kept))
        ),
        (vec!["auditor.md", "linked.md"], vec!["auditor.md"])
    );
}
