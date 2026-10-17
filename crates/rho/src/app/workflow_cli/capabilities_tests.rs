use pretty_assertions::assert_eq;

use super::plugin_agent_sources;

// Covers: the workflow tool reads exactly the plugin files discovery kept
// (never another file in the directory), and an unreadable plugin file skips
// only itself instead of failing every workflow.
// Owner: workflow dynamic capability adapter.
#[test]
fn plugin_agent_sources_read_only_discovered_files_and_skip_bad_ones() {
    let plugin = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(plugin.path()).unwrap();
    std::fs::write(
        dir.join("auditor.md"),
        "---\ndescription: plugin auditor\n---\nprompt\n",
    )
    .unwrap();
    std::fs::write(dir.join("garbled.md"), [0xff, 0xfe]).unwrap();
    std::fs::write(dir.join("left-out.md"), "---\ndescription: x\n---\n").unwrap();

    let sources = plugin_agent_sources(&crate::plugins::PluginAgentFiles {
        files: vec![dir.join("auditor.md"), dir.join("garbled.md")],
        dir: dir.clone(),
    });

    assert_eq!(
        sources
            .iter()
            .map(|source| (source.path.clone(), source.contents.is_ok()))
            .collect::<Vec<_>>(),
        vec![
            (dir.join("auditor.md"), true),
            (dir.join("garbled.md"), false)
        ]
    );
}
