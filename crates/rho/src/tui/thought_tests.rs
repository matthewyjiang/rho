use super::{format_duration, summary};
use pretty_assertions::assert_eq;
use std::time::Duration;

#[test]
fn formats_compact_progressive_durations() {
    assert_eq!(format_duration(Duration::from_millis(0)), "0.0s");
    assert_eq!(format_duration(Duration::from_millis(320)), "0.3s");
    assert_eq!(format_duration(Duration::from_millis(3_200)), "3.2s");
    assert_eq!(format_duration(Duration::from_secs(45)), "45.0s");
    assert_eq!(format_duration(Duration::from_secs(65)), "1m 5s");
    assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
    assert_eq!(format_duration(Duration::from_secs(3_720)), "1h 2m");
}

#[test]
fn summary_prefixes_thought_for() {
    assert_eq!(summary(Duration::from_millis(1_500)), "Thought for 1.5s");
}
