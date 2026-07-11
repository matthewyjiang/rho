use super::{expand, validate, PromptTemplates};

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
