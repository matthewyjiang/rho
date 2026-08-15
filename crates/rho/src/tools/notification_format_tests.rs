use super::notification_format::join_budgeted_sections;

// Covers: a truncated notification body must still tell the model sections were omitted
// Owner: notification format
#[test]
fn join_budgeted_sections_reserves_omission_marker() {
    let body = join_budgeted_sections(["aaaa", "bbbb", "cccc"], "\n", 10, |remaining| {
        format!("...{remaining}")
    });
    assert_eq!(body, "aaaa\n...2");
}
