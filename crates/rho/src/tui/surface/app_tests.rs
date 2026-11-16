use pretty_assertions::assert_eq;

use super::*;

// Covers: a date field must keep text that is not ISO 8601 out of the data
// model the agent reads, and must not refuse a real date.
// Owner: pure parser
#[test]
fn date_time_field_takes_only_iso_8601() {
    let date = FieldKind::DateTime {
        date: true,
        time: false,
    };
    let time = FieldKind::DateTime {
        date: false,
        time: true,
    };
    let both = FieldKind::DateTime {
        date: true,
        time: true,
    };
    let cases = [
        (date, "", true),
        (date, "2026-01-31", true),
        (date, "2026-02-30", false),
        (date, "2026-1-31", false),
        (date, "2026-01-31T10:00", false),
        (time, "09:30", true),
        (time, "09:30:15", true),
        (time, "24:00", false),
        (time, "9:30", false),
        (both, "2026-01-31T09:30", true),
        (both, "2026-01-31T09:30:15", true),
        (both, "2026-01-31", false),
        (FieldKind::Short, "anything", true),
    ];
    for (kind, text, allowed) in cases {
        assert_eq!(kind.problem(text).is_none(), allowed, "{kind:?} {text:?}");
    }
}
