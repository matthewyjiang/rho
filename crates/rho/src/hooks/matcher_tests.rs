use crate::tools::CANONICAL_TOOL_NAMES;
use pretty_assertions::assert_eq;

use super::*;

fn matcher(patterns: &[&str]) -> ToolMatcher {
    ToolMatcher::new(
        patterns
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
        CANONICAL_TOOL_NAMES,
    )
    .expect("pattern list is valid")
}

#[test]
fn an_omitted_tool_list_matches_every_tool() {
    let matcher = ToolMatcher::any();

    assert!(matcher.matches("bash"));
    assert!(matcher.matches("read_file"));
    assert_eq!(matcher.describe(), "*");
}

#[test]
fn exact_names_match_only_themselves() {
    let matcher = matcher(&["bash", "powershell"]);

    assert!(matcher.matches("bash"));
    assert!(matcher.matches("powershell"));
    assert!(!matcher.matches("read_file"));
    assert_eq!(matcher.describe(), "bash, powershell");
}

#[test]
fn a_trailing_star_matches_by_prefix() {
    let matcher = matcher(&["web*"]);

    assert!(matcher.matches("web_search"));
    assert!(!matcher.matches("bash"));
    assert_eq!(matcher.describe(), "web*");
}

#[test]
fn a_bare_star_matches_everything() {
    let matcher = matcher(&["*"]);

    assert!(matcher.matches("bash"));
    assert!(matcher.matches("anything"));
}

#[test]
fn an_empty_list_is_rejected_rather_than_read_as_all_tools() {
    assert_eq!(
        ToolMatcher::new(Vec::new(), CANONICAL_TOOL_NAMES),
        Err(ToolMatcherError::Empty)
    );
}

#[test]
fn a_blank_pattern_is_rejected() {
    assert_eq!(
        ToolMatcher::new(vec!["  ".into()], CANONICAL_TOOL_NAMES),
        Err(ToolMatcherError::BlankPattern)
    );
}

#[test]
fn only_a_single_trailing_star_is_supported() {
    assert_eq!(
        ToolMatcher::new(vec!["*_file".into()], CANONICAL_TOOL_NAMES),
        Err(ToolMatcherError::UnsupportedGlob {
            pattern: "*_file".into()
        })
    );
    assert_eq!(
        ToolMatcher::new(vec!["re*d*".into()], CANONICAL_TOOL_NAMES),
        Err(ToolMatcherError::UnsupportedGlob {
            pattern: "re*d*".into()
        })
    );
}

#[test]
fn a_name_outside_the_canonical_list_fails_at_load() {
    assert_eq!(
        ToolMatcher::new(vec!["Bash".into()], CANONICAL_TOOL_NAMES),
        Err(ToolMatcherError::UnknownTool {
            pattern: "Bash".into()
        })
    );
    assert_eq!(
        ToolMatcher::new(vec!["shell".into()], CANONICAL_TOOL_NAMES),
        Err(ToolMatcherError::UnknownTool {
            pattern: "shell".into()
        })
    );
}

#[test]
fn a_prefix_that_selects_nothing_fails_at_load() {
    assert_eq!(
        ToolMatcher::new(vec!["zzz*".into()], CANONICAL_TOOL_NAMES),
        Err(ToolMatcherError::UnknownTool {
            pattern: "zzz*".into()
        })
    );
}
