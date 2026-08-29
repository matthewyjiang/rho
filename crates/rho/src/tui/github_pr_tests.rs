use pretty_assertions::assert_eq;

use super::{
    classify_gh_pr_view, command_may_change_pr, paint_for_current_branch, parse_gh_pr_view,
    GithubPr, GithubPrLookup, GithubPrPaint, GithubPrProbe, GithubPrTone,
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
fn classify_gh_pr_view_separates_absence_from_transient_failure() {
    // Covers: gh stderr "no pull requests found" clears; other failures do not
    // Owner: github pr probe
    let ready =
        r#"{"number":12,"reviewDecision":null,"mergeStateStatus":"CLEAN","statusCheckRollup":[]}"#;
    let cases = [
        (
            true,
            ready.as_bytes(),
            b"" as &[u8],
            GithubPrProbe::Found(GithubPr {
                number: 12,
                tone: Some(GithubPrTone::Ready),
            }),
        ),
        (
            false,
            b"",
            b"no pull requests found for branch \"feat\"\n",
            GithubPrProbe::Absent,
        ),
        (
            false,
            b"",
            b"GraphQL: no open pull requests found for branch \"feat\"\n",
            GithubPrProbe::Absent,
        ),
        (
            false,
            b"",
            b"gh: To get started with GitHub CLI, please run: gh auth login\n",
            GithubPrProbe::Unavailable,
        ),
        (true, b"not json", b"", GithubPrProbe::Unavailable),
    ];
    for (success, stdout, stderr, expected) in cases {
        assert_eq!(
            classify_gh_pr_view(success, stdout, stderr),
            expected,
            "success={success} stderr={}",
            String::from_utf8_lossy(stderr)
        );
    }
}

#[test]
fn paint_for_current_branch_clears_confirmed_absence_only() {
    // Covers: found paints; no-PR clears; stale/unavailable keep the chip
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
            GithubPrPaint::Show(pr.clone()),
        ),
        (
            Some("main"),
            GithubPrLookup {
                branch: Some("feature".into()),
                probe: GithubPrProbe::Found(pr.clone()),
            },
            GithubPrPaint::Keep,
        ),
        (
            Some("main"),
            GithubPrLookup {
                branch: Some("main".into()),
                probe: GithubPrProbe::Unavailable,
            },
            GithubPrPaint::Keep,
        ),
        (
            Some("main"),
            GithubPrLookup {
                branch: Some("main".into()),
                probe: GithubPrProbe::Absent,
            },
            GithubPrPaint::Clear,
        ),
        (
            Some("main"),
            GithubPrLookup {
                branch: Some("feature".into()),
                probe: GithubPrProbe::Absent,
            },
            GithubPrPaint::Keep,
        ),
    ];
    for (current, lookup, expected) in cases {
        assert_eq!(paint_for_current_branch(current, lookup), expected);
    }
}

#[test]
fn command_may_change_pr_detects_gh_pr_and_git_push() {
    // Covers: gh pr / git push after flags and wrappers refetch; other git/gh do not
    // Owner: github pr probe
    let cases = [
        ("gh pr create --title x", true),
        ("gh --repo o/r pr create", true),
        ("gh --hostname github.com pr merge", true),
        ("/usr/bin/gh pr merge", true),
        ("sudo gh pr create", true),
        ("FOO=1 gh pr view", true),
        ("cd src && gh pr create", true),
        ("git push origin HEAD", true),
        ("git -C repo push", true),
        ("git --no-pager push", true),
        ("git.exe push", true),
        ("gh issue create", false),
        ("git commit -m ready", false),
        ("git --no-pager status", false),
        ("git status", false),
        ("ls", false),
    ];
    for (command, expected) in cases {
        assert_eq!(command_may_change_pr(command), expected, "{command}");
    }
}
