use std::path::Path;

use tempfile::TempDir;

use super::*;

fn write_preset(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn discovers_builtin_presets() {
    let root = TempDir::new().unwrap();

    let presets = discover_with_home(root.path(), None);

    let names: Vec<_> = presets.iter().map(|preset| preset.name.as_str()).collect();
    assert!(names.contains(&"explorer"));
    assert!(names.contains(&"worker"));
    let explorer = presets.iter().find(|p| p.name == "explorer").unwrap();
    assert_eq!(explorer.source, PresetSource::BuiltIn);
    assert!(explorer
        .tools
        .as_ref()
        .unwrap()
        .contains(&"read_file".into()));
    assert_eq!(explorer.reasoning, Some(ReasoningLevel::Low));
    assert!(!explorer.prompt.is_empty());
    let worker = presets.iter().find(|p| p.name == "worker").unwrap();
    assert_eq!(worker.tools, None);
}

#[test]
fn parses_full_frontmatter() {
    let root = TempDir::new().unwrap();
    write_preset(
        root.path(),
        ".rho/agents/reviewer.md",
        "---\ndescription: Reviews diffs\nmodel: some-model\nprovider: anthropic\nreasoning: high\ntools: [read_file, bash]\non_exit: close-on-success\n---\nReview carefully.\n",
    );

    let presets = discover_with_home(root.path(), Some(root.path()));
    let reviewer = presets.iter().find(|p| p.name == "reviewer").unwrap();

    assert_eq!(reviewer.description, "Reviews diffs");
    assert_eq!(reviewer.model.as_deref(), Some("some-model"));
    assert_eq!(reviewer.provider.as_deref(), Some("anthropic"));
    assert_eq!(reviewer.reasoning, Some(ReasoningLevel::High));
    assert_eq!(
        reviewer.tools,
        Some(vec!["read_file".to_string(), "bash".to_string()])
    );
    assert_eq!(reviewer.on_exit, OnExit::CloseOnSuccess);
    assert_eq!(reviewer.prompt, "Review carefully.");
}

#[test]
fn user_preset_overrides_builtin() {
    let root = TempDir::new().unwrap();
    write_preset(
        root.path(),
        ".rho/agents/explorer.md",
        "---\ndescription: custom explorer\n---\nbody\n",
    );

    let presets = discover_with_home(root.path(), Some(root.path()));

    let explorers: Vec<_> = presets.iter().filter(|p| p.name == "explorer").collect();
    assert_eq!(explorers.len(), 1);
    assert_eq!(explorers[0].description, "custom explorer");
    assert!(matches!(explorers[0].source, PresetSource::File(_)));
}

#[test]
fn discovers_project_presets() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write_preset(
        project.path(),
        ".agents/agents/project-agent.md",
        "---\ndescription: project scoped\n---\nbody\n",
    );

    let presets = discover_with_home(project.path(), Some(home.path()));

    assert!(presets.iter().any(|p| p.name == "project-agent"));
}

#[test]
fn rejects_preset_without_description() {
    let root = TempDir::new().unwrap();
    write_preset(
        root.path(),
        ".rho/agents/bad.md",
        "---\nmodel: x\n---\nbody\n",
    );

    let presets = discover_with_home(root.path(), Some(root.path()));

    assert!(!presets.iter().any(|p| p.name == "bad"));
}

#[test]
fn rejects_invalid_names_and_on_exit() {
    let root = TempDir::new().unwrap();
    write_preset(
        root.path(),
        ".rho/agents/Bad--Name.md",
        "---\ndescription: d\n---\n",
    );
    write_preset(
        root.path(),
        ".rho/agents/badexit.md",
        "---\ndescription: d\non_exit: explode\n---\n",
    );

    let presets = discover_with_home(root.path(), Some(root.path()));

    assert!(!presets
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case("bad--name")));
    assert!(!presets.iter().any(|p| p.name == "badexit"));
}

#[test]
fn parses_tool_list_variants() {
    assert_eq!(
        parse_tool_list("[read_file, \"bash\", 'skill']"),
        vec!["read_file", "bash", "skill"]
    );
    assert_eq!(
        parse_tool_list("read_file, bash"),
        vec!["read_file", "bash"]
    );
    assert_eq!(parse_tool_list("[]"), Vec::<String>::new());
}

#[test]
fn find_reports_unknown_preset() {
    let root = TempDir::new().unwrap();

    let error = find(root.path(), "nonexistent").unwrap_err();

    assert!(error.to_string().contains("unknown subagent preset"));
}

#[test]
fn status_file_roundtrip() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested").join("result.json");
    let status = RunStatus {
        state: RunState::Running,
        pid: Some(42),
        preset: Some("explorer".into()),
        turns: 3,
        input_tokens: 1000,
        output_tokens: 50,
        last_activity: Some("tool: bash".into()),
        last_text: Some("found it".into()),
        result: None,
        error: None,
    };

    write_status(&path, &status).unwrap();

    assert_eq!(read_status(&path), Some(status));
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn read_status_tolerates_missing_and_invalid_files() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("result.json");

    assert_eq!(read_status(&path), None);

    std::fs::write(&path, "not json").unwrap();
    assert_eq!(read_status(&path), None);
}

#[test]
fn run_state_terminality() {
    assert!(!RunState::Starting.is_terminal());
    assert!(!RunState::Running.is_terminal());
    assert!(RunState::Ok.is_terminal());
    assert!(RunState::Error.is_terminal());
    assert!(RunState::Stopped.is_terminal());
}
