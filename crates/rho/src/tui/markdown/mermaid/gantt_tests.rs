use mermaid_rs_renderer::ir::GanttTask;
use pretty_assertions::assert_eq;

use super::{parse_gantt_date, parse_gantt_duration, schedule, GanttRow};

fn task(
    id: &str,
    label: &str,
    start: Option<&str>,
    duration: Option<&str>,
    after: Option<&str>,
) -> GanttTask {
    GanttTask {
        id: id.to_string(),
        label: label.to_string(),
        start: start.map(str::to_string),
        duration: duration.map(str::to_string),
        after: after.map(str::to_string),
        section: None,
        status: None,
    }
}

fn task_starts(model: &super::GanttModel) -> Vec<(&str, i32, i32)> {
    model
        .rows
        .iter()
        .filter_map(|row| match row {
            GanttRow::Task {
                label,
                start,
                duration,
                ..
            } => Some((
                label.as_str(),
                start.round() as i32,
                duration.round() as i32,
            )),
            GanttRow::Section(_) => None,
        })
        .collect()
}

// Covers: gantt date/duration tokens and after-chains must schedule in day units
// Owner: mermaid gantt scheduler
#[test]
fn schedules_dates_after_chains_and_duration_units() {
    assert_eq!(parse_gantt_duration("2d"), Some(2.0));
    assert_eq!(parse_gantt_duration("1w"), Some(7.0));
    assert!(parse_gantt_date("2026-01-01").is_some());
    assert_eq!(parse_gantt_date("soon"), None);

    let dated = schedule(
        &[
            task("p1", "Parser", Some("2026-01-01"), Some("3d"), None),
            task("p2", "Painter", None, Some("2d"), Some("p1")),
        ],
        None,
    );
    assert!(dated.has_dates);
    let starts = task_starts(&dated);
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0].0, "Parser");
    assert_eq!(starts[0].2, 3);
    assert_eq!(starts[1], ("Painter", starts[0].1 + 3, 2));

    let relative = schedule(
        &[
            task("a", "One", None, Some("2d"), None),
            task("b", "Two", None, Some("2d"), Some("a")),
        ],
        None,
    );
    assert!(!relative.has_dates);
    assert_eq!(task_starts(&relative), vec![("One", 0, 2), ("Two", 2, 2)]);

    let fallback = schedule(&[task("a", "Soon", Some("soon"), Some("2d"), None)], None);
    assert!(!fallback.has_dates);
    assert_eq!(task_starts(&fallback), vec![("Soon", 0, 2)]);
}
