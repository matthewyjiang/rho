use std::time::Duration;

use pretty_assertions::assert_eq;

use super::parse_duration;

#[test]
fn parses_human_durations_and_rejects_invalid_values() {
    assert_eq!(parse_duration("20m").unwrap(), Duration::from_secs(1_200));
    assert!(parse_duration("soon")
        .unwrap_err()
        .contains("invalid duration"));
}
