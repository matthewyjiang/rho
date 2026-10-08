use super::{expand, matches_search, merge, usage, validate, PromptTemplates};
use pretty_assertions::assert_eq;

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

// Covers: only namespaced commands resolve templates, regardless of prefix case;
// short or non-ASCII command names must not panic on byte boundaries.
// Owner: prompt template command parsing
#[test]
fn template_command_prefix_is_case_insensitive_and_utf8_safe() {
    for (command, expected) in [
        ("prompt:review", Some("review")),
        ("PrOmPt:Review", Some("Review")),
        ("prompt:", Some("")),
        ("review", None),
        ("prompt", None),
        ("prompted:review", None),
        ("skill:review", None),
        ("💡💡", None),
    ] {
        assert_eq!(super::command_template_name(command), expected, "{command}");
    }
}

#[test]
fn expands_arguments_into_placeholders_or_appends_them() {
    let cases = [
        // (template, trailing text, expanded)
        ("Review this.", "  src/a.rs ", "Review this. src/a.rs"),
        ("Review this.", "", "Review this."),
        (
            "Literal $0, $x, and $.",
            "text",
            "Literal $0, $x, and $. text",
        ),
        ("Review $ARGUMENTS now.", "a b", "Review a b now."),
        ("Compare $2 with $1.", "old new", "Compare new with old."),
        (
            "Fix $1 in $2.",
            "\"the login bug\" src/auth.rs",
            "Fix the login bug in src/auth.rs.",
        ),
        ("Explain $1 and $3.", "only-one", "Explain only-one and ."),
        ("Fix $1.", "it's broken", "Fix it's."),
        ("$1: costs $0 and $x", "a", "a: costs $0 and $x"),
        ("Use $10.", "1 2 3 4 5 6 7 8 9 ten", "Use ten."),
    ];

    for (template, trailing, expected) in cases {
        assert_eq!(
            expand(template, trailing),
            expected,
            "{template:?} {trailing:?}"
        );
    }
}

#[test]
fn usage_names_the_arguments_a_template_places() {
    let cases = [
        // (template, usage)
        ("Review this.", "/prompt:t [text]"),
        ("Costs $0 and $x.", "/prompt:t [text]"),
        ("Fix $1.", "/prompt:t [$1]"),
        ("Compare $2 with $1, again $2.", "/prompt:t [$1] [$2]"),
        ("Only $3.", "/prompt:t [$1] [$2] [$3]"),
        ("Focus: $ARGUMENTS", "/prompt:t [arguments]"),
        ("Fix $1. Notes: $ARGUMENTS", "/prompt:t [$1] [arguments]"),
        ("Typo $100.", "/prompt:t [$1] … [$100]"),
    ];

    for (template, expected) in cases {
        assert_eq!(usage("prompt:t", template), expected, "{template:?}");
    }
}
