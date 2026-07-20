use tempfile::TempDir;

use super::*;

#[test]
fn formats_agent_metadata_with_prompt_extension_preview() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let directory = home.path().join(".rho/agents");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("release-reviewer.md"),
        "---\ndescription: Reviews releases before deployment.\nprompt: extend\nmodel-policy: require\nprovider: anthropic\nmodel: claude-sonnet\nreasoning: high\ntools: [read_file, bash]\n---\nSECRET PROMPT BODY\n",
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
    assert!(detail.contains("extend system prompt"));
    assert!(detail.contains("Prompt extension preview"));
    assert!(detail.contains("SECRET PROMPT BODY"));
}

#[test]
fn truncates_long_prompt_previews_at_character_boundaries() {
    let prompt = format!("{}suffix", "🦀".repeat(PROMPT_PREVIEW_MAX_CHARS));

    let preview = prompt_preview(&prompt);

    assert_eq!(preview.matches('🦀').count(), PROMPT_PREVIEW_MAX_CHARS);
    assert!(preview.ends_with("\n… (preview truncated)"));
    assert!(!preview.contains("suffix"));
}
