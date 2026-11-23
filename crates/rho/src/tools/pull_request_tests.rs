use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: a call names one pull request as a non-empty string; anything
// else is refused as invalid arguments before the inbox is asked.
// Owner: pull request tools.
#[test]
fn the_pull_request_argument_is_a_non_empty_string() {
    let invalid = Err(ToolErrorKind::InvalidArguments);
    for (arguments, expected) in [
        (
            json!({"pull_request": "https://github.com/acme/widgets/pull/12"}),
            Ok("https://github.com/acme/widgets/pull/12"),
        ),
        (
            json!({"pull_request": "acme/widgets#12"}),
            Ok("acme/widgets#12"),
        ),
        (json!({"pull_request": "  #12\n"}), Ok("#12")),
        (json!({}), invalid),
        (json!({"pull_request": ""}), invalid),
        (json!({"pull_request": "   "}), invalid),
        (json!({"pull_request": 12}), invalid),
        (json!({"pull_request": null}), invalid),
    ] {
        assert_eq!(
            target(&arguments).map_err(|error| error.kind()),
            expected,
            "{arguments}"
        );
    }
}

// Covers: each refusal becomes a tool error of the right kind whose text
// carries the facts the agent needs to act: the limit and the asked value,
// the budget and its reset, the host to sign in to, or the forge's words.
// Owner: pull request tools.
#[test]
fn refusals_keep_their_facts() {
    use ToolErrorKind::{Execution, InvalidArguments};
    for (error, kind, facts) in [
        (
            PullRequestError::TooManyWatches {
                limit: 25,
                asked: 26,
            },
            Execution,
            vec!["25", "26"],
        ),
        (
            PullRequestError::Budget {
                remaining: 43,
                floor: 50,
                // 2027-01-15 08:00 UTC.
                resets_at: 1_800_000_000,
            },
            Execution,
            vec!["43", "50", "2027-01-15 08:00"],
        ),
        (
            PullRequestError::SignedOut {
                host: "ghe.example.com".to_owned(),
            },
            Execution,
            vec!["gh auth login --hostname ghe.example.com"],
        ),
        (
            PullRequestError::AlreadyEnded {
                state: PrState::Merged,
            },
            Execution,
            vec!["merged"],
        ),
        (PullRequestError::NotInstalled, Execution, vec![]),
        (
            PullRequestError::NotFound("no pull request acme/widgets#9".to_owned()),
            Execution,
            vec!["no pull request acme/widgets#9"],
        ),
        (
            PullRequestError::Read("gh didn't answer within 60 s".to_owned()),
            Execution,
            vec!["gh didn't answer within 60 s"],
        ),
        (
            PullRequestError::Store("disk full".to_owned()),
            Execution,
            vec!["disk full"],
        ),
        (
            PullRequestError::InvalidTarget("no repository".to_owned()),
            InvalidArguments,
            vec!["no repository"],
        ),
    ] {
        let tool_error = refused(error.clone());
        let missing: Vec<&str> = facts
            .iter()
            .copied()
            .filter(|fact| !tool_error.message().contains(fact))
            .collect();
        assert_eq!(
            (tool_error.kind(), missing),
            (kind, Vec::new()),
            "{error:?}: {}",
            tool_error.message()
        );
    }
}
