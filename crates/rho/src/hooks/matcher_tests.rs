use crate::tools::canonical_tool_names;
use pretty_assertions::assert_eq;

use super::*;

fn matcher(patterns: &[&str]) -> ToolMatcher {
    ToolMatcher::new(
        patterns
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
        canonical_tool_names(),
    )
    .expect("pattern list is valid")
}

// Covers: hook tool patterns match exact names or a trailing-star prefix, and
// an omitted list matches every tool
// Owner: hook tool matcher
#[test]
fn patterns_match_exact_names_or_a_trailing_star_prefix() {
    let any = ToolMatcher::any();
    for (case, tools, tool, expected) in [
        ("omitted list", &any, "read_file", true),
        (
            "exact name",
            &matcher(&["bash", "powershell"]),
            "powershell",
            true,
        ),
        (
            "exact name only",
            &matcher(&["bash", "powershell"]),
            "read_file",
            false,
        ),
        ("trailing star", &matcher(&["web*"]), "web_search", true),
        (
            "trailing star prefix only",
            &matcher(&["web*"]),
            "bash",
            false,
        ),
        ("bare star", &matcher(&["*"]), "anything", true),
    ] {
        assert_eq!(tools.matches(tool), expected, "{case}");
    }
}

#[test]
fn an_unsupported_glob_or_unknown_name_fails_at_load() {
    let unsupported = |pattern: &str| ToolMatcherError::UnsupportedGlob {
        pattern: pattern.into(),
    };
    let unknown = |pattern: &str| ToolMatcherError::UnknownTool {
        pattern: pattern.into(),
    };
    for (case, patterns, expected) in [
        (
            "empty list is not all tools",
            vec![],
            ToolMatcherError::Empty,
        ),
        (
            "blank pattern",
            vec!["  ".into()],
            ToolMatcherError::BlankPattern,
        ),
        ("leading star", vec!["*_file".into()], unsupported("*_file")),
        ("inner star", vec!["re*d*".into()], unsupported("re*d*")),
        ("wrong case", vec!["Bash".into()], unknown("Bash")),
        ("non-canonical name", vec!["shell".into()], unknown("shell")),
        (
            "prefix selecting nothing",
            vec!["zzz*".into()],
            unknown("zzz*"),
        ),
    ] {
        assert_eq!(
            ToolMatcher::new(patterns, canonical_tool_names()),
            Err(expected),
            "{case}"
        );
    }
}

// Covers: the hooks overlay labels a matcher with its patterns as written.
// Owner: hook tool matcher
#[test]
fn the_description_lists_patterns_as_written() {
    for (tools, expected) in [
        (ToolMatcher::any(), "*"),
        (matcher(&["bash", "powershell"]), "bash, powershell"),
        (matcher(&["web*"]), "web*"),
    ] {
        assert_eq!(tools.describe(), expected);
    }
}

// Covers: a hook naming only some edit tools reports the ones it misses, since
// the session's edit tool changes with the provider and `behavior.edit_tool`.
// Owner: hook tool matcher
#[test]
fn partial_edit_tool_lists_report_missing_edit_tools() {
    let cases: [(&[&str], &[&str]); 5] = [
        (&["edit", "write"], &["str_replace", "apply_patch"]),
        (&["str_replace"], &["apply_patch", "edit"]),
        (&["edit", "apply_patch", "str_replace"], &[]),
        (&["bash", "write"], &[]),
        (&["*"], &[]),
    ];
    for (patterns, missing) in cases {
        assert_eq!(
            matcher(patterns).missing_edit_tools(),
            missing.to_vec(),
            "{patterns:?}"
        );
    }
    assert_eq!(ToolMatcher::any().missing_edit_tools(), Vec::<&str>::new());
}
