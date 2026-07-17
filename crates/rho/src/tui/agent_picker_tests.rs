use tempfile::TempDir;

use super::*;

#[test]
fn formats_agent_metadata_without_prompt_contents() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let directory = home.path().join(".rho/agents");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("release-reviewer.md"),
        "---\ndescription: Reviews releases before deployment.\nprompt: replace\nmodel-policy: require\nprovider: anthropic\nmodel: claude-sonnet\nreasoning: high\ntools: [read_file, bash]\n---\nSECRET PROMPT BODY\n",
    )
    .unwrap();

    let catalog = AgentCatalog::discover_with_home(cwd.path(), Some(home.path())).unwrap();
    let picker = agent_picker(catalog);
    let item = picker
        .items
        .iter()
        .find(|item| item.value == "release-reviewer")
        .unwrap();
    let detail = item.detail.as_deref().unwrap();

    assert_eq!(picker.layout, PickerLayout::MasterDetail);
    assert!(detail.contains("Reviews releases before deployment."));
    assert!(detail.contains("~/.rho/agents"));
    assert!(detail.contains("require anthropic/claude-sonnet"));
    assert!(detail.contains("high"));
    assert!(detail.contains("bash, read_file"));
    assert!(detail.contains("replace system prompt"));
    assert!(!detail.contains("SECRET PROMPT BODY"));
}
