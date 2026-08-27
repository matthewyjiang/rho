use pretty_assertions::assert_eq;

use super::{
    parse_gh_pr_view, pr_for_current_branch, GithubPr, GithubPrLookup, GithubPrProbe, GithubPrTone,
};

#[test]
fn parse_gh_pr_view_maps_ready_and_issue_tones() {
    // Covers: statusline color is ready vs issues vs ambient from gh JSON
    // Owner: github pr probe
    let cases = [
        (
            r#"{"number":12,"reviewDecision":null,"mergeStateStatus":"CLEAN","statusCheckRollup":[]}"#,
            Some(GithubPr {
                number: 12,
                tone: Some(GithubPrTone::Ready),
            }),
        ),
        (
            r#"{"number":4,"reviewDecision":"CHANGES_REQUESTED","mergeStateStatus":"BLOCKED","statusCheckRollup":[]}"#,
            Some(GithubPr {
                number: 4,
                tone: Some(GithubPrTone::Issues),
            }),
        ),
        (
            r#"{"number":8,"reviewDecision":null,"mergeStateStatus":"DIRTY","statusCheckRollup":[]}"#,
            Some(GithubPr {
                number: 8,
                tone: Some(GithubPrTone::Issues),
            }),
        ),
        (
            r#"{"number":9,"reviewDecision":null,"mergeStateStatus":"UNSTABLE","statusCheckRollup":[{"conclusion":"FAILURE","state":""}]}"#,
            Some(GithubPr {
                number: 9,
                tone: Some(GithubPrTone::Issues),
            }),
        ),
        (
            r#"{"number":3,"reviewDecision":null,"mergeStateStatus":"BLOCKED","statusCheckRollup":null}"#,
            Some(GithubPr {
                number: 3,
                tone: None,
            }),
        ),
        (
            r#"{"number":5,"reviewDecision":null,"mergeStateStatus":"CLEAN","statusCheckRollup":{"not":"an array"}}"#,
            Some(GithubPr {
                number: 5,
                tone: Some(GithubPrTone::Ready),
            }),
        ),
        ("not json", None),
        (r#"{"reviewDecision":null}"#, None),
    ];
    for (json, expected) in cases {
        assert_eq!(parse_gh_pr_view(json.as_bytes()), expected, "{json}");
    }
}

#[test]
fn pr_for_current_branch_ignores_stale_or_unavailable_lookups() {
    // Covers: another branch, or a failed/no-PR probe, must not paint or clear
    // Owner: github pr probe
    let pr = GithubPr {
        number: 7,
        tone: Some(GithubPrTone::Ready),
    };
    let cases = [
        (
            Some("main"),
            GithubPrLookup {
                branch: Some("main".into()),
                probe: GithubPrProbe::Found(pr.clone()),
            },
            Some(pr.clone()),
        ),
        (
            Some("main"),
            GithubPrLookup {
                branch: Some("feature".into()),
                probe: GithubPrProbe::Found(pr.clone()),
            },
            None,
        ),
        (
            Some("main"),
            GithubPrLookup {
                branch: Some("main".into()),
                probe: GithubPrProbe::Unavailable,
            },
            None,
        ),
    ];
    for (current, lookup, expected) in cases {
        assert_eq!(pr_for_current_branch(current, lookup), expected);
    }
}
