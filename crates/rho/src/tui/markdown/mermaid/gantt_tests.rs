use mermaid_rs_renderer::ir::GanttTask;
use pretty_assertions::assert_eq;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::tui::terminal_graph::GraphStyles;

use super::{layout_gantt, parse_gantt_date, parse_gantt_duration, schedule, GanttAxis, GanttRow};

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

fn task_timing(model: &super::GanttModel, name: &str) -> (f32, f32) {
    model
        .rows
        .iter()
        .find_map(|row| match row {
            GanttRow::Task {
                label,
                start,
                duration,
                ..
            } if label == name => Some((*start, *duration)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing task {name}"))
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

fn styles() -> GraphStyles {
    GraphStyles {
        border: Style::default(),
        node_text: Style::default(),
        edge: Style::default(),
        edge_label: Style::default(),
        node_styles: Vec::new(),
    }
}

// Covers: gantt date/duration tokens and after-chains must schedule in day units
// Owner: mermaid gantt scheduler
#[test]
fn schedules_dates_after_chains_and_duration_units() {
    assert_eq!(parse_gantt_duration("2d"), Some(2.0));
    assert_eq!(parse_gantt_duration("1w"), Some(7.0));
    assert_eq!(parse_gantt_duration("90m"), Some(90.0 / 1_440.0));
    assert_eq!(parse_gantt_duration("1M"), Some(30.0));
    assert_eq!(parse_gantt_duration("500ms"), Some(500.0 / 86_400_000.0));
    assert!(parse_gantt_date("2026-01-01").is_some());
    assert_eq!(parse_gantt_date("soon"), None);

    let dated = schedule(
        &[
            task("p1", "Parser", Some("2026-01-01"), Some("3d"), None),
            task("p2", "Painter", None, Some("2d"), Some("p1")),
        ],
        None,
    );
    assert!(matches!(dated.axis, GanttAxis::Calendar { .. }));
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
    assert!(matches!(relative.axis, GanttAxis::Relative { .. }));
    assert_eq!(task_starts(&relative), vec![("One", 0, 2), ("Two", 2, 2)]);

    let fallback = schedule(&[task("a", "Soon", Some("soon"), Some("2d"), None)], None);
    assert!(matches!(fallback.axis, GanttAxis::Relative { .. }));
    assert_eq!(task_starts(&fallback), vec![("Soon", 0, 2)]);

    let mixed_case = schedule(
        &[
            task("TaskA", "Parser", None, Some("2d"), None),
            task("p2", "Painter", None, Some("2d"), Some("taska")),
        ],
        None,
    );
    assert_eq!(
        task_starts(&mixed_case),
        vec![("Parser", 0, 2), ("Painter", 2, 2)]
    );

    let subday = schedule(
        &[
            task("p1", "Parser", Some("2026-01-01"), Some("90m"), None),
            task("p2", "Painter", None, Some("2d"), Some("p1")),
        ],
        None,
    );
    let (parser_start, parser_duration) = task_timing(&subday, "Parser");
    let (painter_start, _) = task_timing(&subday, "Painter");
    assert!((parser_duration - 90.0 / 1_440.0).abs() < 1e-6);
    assert!((painter_start - (parser_start + parser_duration)).abs() < 1e-6);
}

// Covers: short task names must reserve the label floor and fit mid-width panes
// Owner: mermaid gantt layout
#[test]
fn short_labels_fit_mid_width_panes() {
    let model = schedule(
        &[
            task("p1", "Parser", None, Some("3d"), None),
            task("p2", "Painter", None, Some("2d"), Some("p1")),
        ],
        Some("Plan".to_string()),
    );
    let art = layout_gantt(&model, &styles(), Some(50)).expect("short labels should fit");
    assert!(
        art.plain_lines.iter().all(|line| line.width() <= 50),
        "{:?}",
        art.plain_lines
    );
    assert!(art.plain_lines.iter().any(|line| line.contains("Parser")));
}
