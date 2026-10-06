use pretty_assertions::assert_eq;

use super::prompt_history_items;

// Covers: newest first, so the most recent match highlights first; a repeated
// prompt shows once at its latest use; multi-line prompts get one-line labels.
// Owner: prompt history search rows.
#[test]
fn rows_are_newest_first_without_repeats() {
    let history = [
        "fix the build",
        "run tests\n  again",
        "fix the build",
        "ship it",
    ]
    .map(String::from);
    let rows = prompt_history_items(&history)
        .into_iter()
        .map(|item| (item.label, item.value))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            ("ship it", "ship it"),
            ("fix the build", "fix the build"),
            ("run tests again", "run tests\n  again"),
        ]
        .map(|(label, value)| (label.to_string(), value.to_string()))
    );
}
