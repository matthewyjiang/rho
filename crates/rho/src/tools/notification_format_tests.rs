use pretty_assertions::assert_eq;

use super::notification_format::{
    bounded_notification, join_budgeted_sections, Excerpt, MODEL_NOTIFICATION_BYTES,
};

// Covers: a truncated notification body must still tell the model sections were omitted
// Owner: notification format
#[test]
fn join_budgeted_sections_reserves_omission_marker() {
    let body = join_budgeted_sections(["aaaa", "bbbb", "cccc"], "\n", 10, |remaining| {
        format!("...{remaining}")
    });
    assert_eq!(body, "aaaa\n...2");
}

// Covers: excerpts too long for one notification are cut so the whole text
// fits the notification budget, every summary still shows ahead of them,
// and every excerpt that starts also closes. Otherwise one long result
// hides how the others ended, the wake overflows its budget, or an open
// fence swallows the footer.
// Owner: notification format
#[test]
fn long_excerpts_are_cut_to_the_notification_budget() {
    let long = "x".repeat(MODEL_NOTIFICATION_BYTES);
    let names = ["a", "b", "c"];
    let model = bounded_notification(
        "header\n",
        "\nfooter",
        names.iter().map(|name| format!("summary {name}")),
        |remaining| format!("{remaining} more"),
        "\nexcerpts:",
        names
            .iter()
            .map(|name| Excerpt {
                label: format!("\n{name}:\n<<"),
                text: &long,
                close: ">>".into(),
            })
            .collect(),
    );
    let summaries: Vec<&str> = model
        .lines()
        .filter(|line| line.starts_with("summary "))
        .collect();

    assert_eq!(
        (
            model.len() <= MODEL_NOTIFICATION_BYTES,
            summaries,
            model.matches("<<").count() == model.matches(">>").count(),
            model.ends_with("\nfooter"),
        ),
        (
            true,
            vec!["summary a", "summary b", "summary c"],
            true,
            true
        ),
        "{}-byte text over the {MODEL_NOTIFICATION_BYTES}-byte budget",
        model.len()
    );
}
