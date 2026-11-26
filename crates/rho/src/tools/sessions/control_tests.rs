use pretty_assertions::assert_eq;

use super::*;

// Covers: actions that describe an argument with the same text share one
// clause naming all of them, so the model reads the text once, while actions
// whose text differs keep their own clause, after the history's description.
// Owner: sessions tool schema, merging the core's argument descriptions.
#[test]
fn describe_says_each_distinct_text_once() {
    let uses = |pairs: &[(&'static str, &str)]| -> Vec<(&'static str, String)> {
        pairs
            .iter()
            .map(|(action, text)| (*action, (*text).to_owned()))
            .collect()
    };
    let cases = [
        (
            Some("History."),
            uses(&[
                ("read", "Id."),
                ("prompt", "Id."),
                ("answer", "Own."),
                ("wait", "Id."),
            ]),
            "History. For read, prompt, wait: Id. For answer: Own.",
        ),
        (
            Some("History"),
            uses(&[("list", "Path"), ("new", "Dir.")]),
            "History. For list: Path. For new: Dir.",
        ),
        (None, uses(&[("new", "Prompt.")]), "For new: Prompt."),
        (
            Some(""),
            uses(&[("list", "A."), ("new", "A.")]),
            "For list, new: A.",
        ),
    ];

    let described: Vec<String> = cases
        .iter()
        .map(|(before, uses, _)| describe(*before, uses))
        .collect();
    let expected: Vec<&str> = cases.iter().map(|(_, _, expected)| *expected).collect();

    assert_eq!(described, expected);
}

// Covers: the tool's concurrent-sessions cap is `behavior.agent_concurrency`,
// as a session's config sets it, or as the budget the daemon shares stands
// when the cap is checked, after a config publish resized it.
// Owner: sessions tool, sizing its session-control core.
#[test]
fn max_sessions_follows_agent_concurrency() {
    let shared = AgentConcurrency::from_config(10);
    let live = max_sessions(/*agent_concurrency*/ 10, Some(&shared));
    shared.set_total(32);

    let limits = [
        max_sessions(crate::config::DEFAULT_AGENT_CONCURRENCY, None).limit(),
        max_sessions(/*agent_concurrency*/ 32, None).limit(),
        live.limit(),
    ];

    assert_eq!(limits, [10, 32, 32]);
}
