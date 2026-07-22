use tempfile::TempDir;

use {super::*, crate::config::Config};

#[test]
fn formats_agent_metadata_with_prompt_extension() {
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
    let picker = agent_picker(catalog, AgentModelView::from(&Config::default()));
    let item = picker
        .items
        .iter()
        .find(|item| item.value == "release-reviewer")
        .unwrap();
    let detail = item.detail.as_deref().unwrap();

    assert_eq!(picker.layout, PickerLayout::Overlay);
    assert!(picker.is_overlay());
    let chrome = picker.overlay_chrome.as_ref().unwrap();
    assert_eq!(chrome.nav_label, " AGENTS");
    assert_eq!(chrome.detail_label.as_deref(), Some(" DETAILS"));
    assert_eq!(chrome.nav_keys_hint, "↑↓ agents");
    assert!(detail.contains("Reviews releases before deployment."));
    assert!(detail.contains("~/.rho/agents"));
    assert!(detail.contains("require anthropic/claude-sonnet"));
    assert!(detail.contains("high"));
    assert!(detail.contains("bash, read_file"));
    assert!(detail.contains("extend system prompt"));
    assert!(detail.contains("Prompt extension"));
    assert!(detail.contains("SECRET PROMPT BODY"));
}

#[test]
fn marks_internal_agents_as_reserved() {
    let root = TempDir::new().unwrap();
    let catalog = AgentCatalog::discover_with_home(root.path(), None).unwrap();

    let picker = agent_picker(catalog, AgentModelView::from(&Config::default()));
    let internal_items = picker
        .items
        .iter()
        .filter(|item| matches!(item.value.as_str(), "goal-judge" | "session-title"))
        .collect::<Vec<_>>();

    assert_eq!(internal_items.len(), 2);
    for item in internal_items {
        let badge = item.badge.as_ref().unwrap();
        assert_eq!(badge.text, "(internal)");
        assert_eq!(badge.tone, PickerBadgeTone::Internal);
        let detail = item.detail.as_deref().unwrap();
        assert!(detail.contains("Source\ninternal"));
        assert!(detail.contains("reserved; cannot be overridden or delegated"));
        assert!(detail.contains("Model\nopenai/gpt-5.5"));
        assert!(detail.contains("Model source: conversation fallback"));
        assert!(detail.contains("Reasoning\nlow"));
        assert!(detail.contains("Tools\nnone"));
    }
}

#[test]
fn internal_agent_detail_shows_override_and_source() {
    let root = TempDir::new().unwrap();
    let catalog = AgentCatalog::discover_with_home(root.path(), None).unwrap();
    let mut config = Config::default();
    config.set_internal_agent_model(
        "goal-judge",
        "anthropic".into(),
        "claude-haiku-4-5".into(),
        "anthropic-api-key".into(),
    );

    let picker = agent_picker(catalog, AgentModelView::from(&config));
    let detail = picker
        .items
        .iter()
        .find(|item| item.value == "goal-judge")
        .unwrap()
        .detail
        .as_deref()
        .unwrap();

    assert!(detail.contains("Model\nanthropic/claude-haiku-4-5"));
    assert!(detail.contains("Model source: override"));
}

#[test]
fn internal_agent_model_picker_starts_with_conversation_choice() {
    let picker = crate::tui::model_picker::internal_agent_model_picker(
        "goal-judge",
        "openai",
        "gpt-5.5",
        true,
        &[],
        &[],
    );

    assert_eq!(picker.action, PickerAction::SelectInternalAgentModel);
    assert_eq!(picker.items[0].label, "Use conversation model");
    assert_eq!(
        picker.items[0].value,
        crate::tui::model_picker::USE_CONVERSATION_MODEL
    );
    assert_eq!(picker.selected, 0);
}

#[test]
fn unavailable_internal_agent_override_keeps_conversation_choice_selectable() {
    let picker = crate::tui::model_picker::internal_agent_model_picker(
        "goal-judge",
        "anthropic",
        "unavailable-model",
        false,
        &[],
        &[],
    );

    assert_eq!(picker.items.len(), 1);
    assert_eq!(picker.selected, 0);
    assert_eq!(
        picker.selected_item().unwrap().value,
        crate::tui::model_picker::USE_CONVERSATION_MODEL
    );
}
