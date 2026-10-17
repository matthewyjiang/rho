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

// Covers: the workflow tool reads discovery-vetted plugin agent files without
// asking the permission gate (which would prompt for, or in plan mode deny,
// every installed plugin's agents on each call), while user agent
// directories still ask it; and an unreadable plugin file skips only itself
// instead of failing every workflow.
// Owner: workflow dynamic capability adapter.
#[tokio::test]
async fn plugin_agent_reads_skip_the_gate_that_user_agent_dirs_ask() {
    let plugin = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(plugin.path()).unwrap();
    std::fs::write(
        dir.join("auditor.md"),
        "---\ndescription: plugin auditor\n---\nprompt\n",
    )
    .unwrap();
    std::fs::write(dir.join("garbled.md"), [0xff, 0xfe]).unwrap();
    std::fs::write(dir.join("left-out.md"), "---\ndescription: x\n---\n").unwrap();
    let kept = [dir.join("auditor.md"), dir.join("garbled.md")];
    let service = super::super::AppWorkflowToolService {
        cwd: "/workspace".into(),
        config_path: None,
        tracker: crate::tools::workflow_tracker::WorkflowRunTracker::new(),
    };
    let (progress, _receiver) = rho_sdk::tool::tool_progress_channel(std::num::NonZeroUsize::MIN);
    // The SDK's standalone context denies every capability request.
    let denied = rho_sdk::tool::ToolContext::new(
        /*workspace*/ None,
        rho_sdk::CancellationToken::new(),
        progress,
    );

    let plugin_sources = service
        .authorized_agent_sources(&denied, &dir, AgentFiles::Discovered(&kept))
        .await
        .unwrap();
    let user_dir = service
        .authorized_agent_sources(&denied, &dir, AgentFiles::AllMarkdown)
        .await;

    assert_eq!(
        (
            plugin_sources
                .iter()
                .map(|source| (source.path.clone(), source.contents.is_ok()))
                .collect::<Vec<_>>(),
            user_dir.is_err()
        ),
        (
            vec![
                (dir.join("auditor.md"), true),
                (dir.join("garbled.md"), false)
            ],
            true
        )
    );
}
