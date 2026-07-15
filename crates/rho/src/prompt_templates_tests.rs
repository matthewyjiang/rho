use super::{description, expand, matches_search, merge, validate, PromptTemplates};

#[test]
fn appends_trailing_text_to_template() {
    assert_eq!(
        expand("Review this code.", " src/config.rs "),
        "Review this code. src/config.rs"
    );
}

#[test]
fn validates_names_and_builtin_conflicts() {
    let mut templates = PromptTemplates::new();
    templates.insert("code review".into(), "Review this code.".into());
    assert!(validate(&templates)
        .unwrap_err()
        .to_string()
        .contains("invalid"));

    templates.clear();
    templates.insert("model".into(), "Choose a model.".into());
    assert!(validate(&templates)
        .unwrap_err()
        .to_string()
        .contains("conflicts"));
}

#[test]
fn rejects_case_insensitive_duplicate_names() {
    let templates = PromptTemplates::from([
        ("Review".into(), "upper".into()),
        ("review".into(), "lower".into()),
    ]);

    assert!(validate(&templates)
        .unwrap_err()
        .to_string()
        .contains("case-insensitive"));
}

#[test]
fn merges_case_insensitive_overrides() {
    let mut templates = PromptTemplates::from([("Review".into(), "global".into())]);

    merge(
        &mut templates,
        PromptTemplates::from([("review".into(), "project".into())]),
    );

    assert_eq!(
        templates,
        PromptTemplates::from([("review".into(), "project".into())])
    );
}

#[test]
fn discovers_global_and_project_template_files() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let nested = project.path().join("nested");
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(home.path().join(".rho/prompts")).unwrap();
    std::fs::create_dir_all(project.path().join(".rho/prompts")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(home.path().join(".rho/prompts/review.md"), "global review").unwrap();
    std::fs::write(
        project.path().join(".rho/prompts/review.md"),
        "project review\n",
    )
    .unwrap();
    std::fs::write(
        project.path().join(".rho/prompts/explain.txt"),
        "explain this",
    )
    .unwrap();
    std::fs::write(project.path().join(".rho/prompts/ignored.json"), "ignored").unwrap();

    let templates = super::discover_with_home(&nested, Some(home.path()));

    assert_eq!(
        templates.get("review").map(String::as_str),
        Some("project review")
    );
    assert_eq!(
        templates.get("explain").map(String::as_str),
        Some("explain this")
    );
    assert_eq!(templates.len(), 2);
}

#[test]
fn matches_search_by_prompt_prefix_or_bare_name() {
    assert!(matches_search("review-code", "prompt:rev"));
    assert!(matches_search("review-code", "rev"));
    assert!(!matches_search("review-code", "explain"));
}

#[test]
fn description_previews_normalized_template_contents() {
    assert_eq!(
        description(
            "Review this code for:
- correctness
- security"
        ),
        "Review this code for: - correctness - security"
    );
}
