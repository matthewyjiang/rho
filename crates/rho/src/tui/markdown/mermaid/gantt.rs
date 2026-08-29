use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};
use mermaid_rs_renderer::ir::{GanttStatus, GanttTask};
use unicode_width::UnicodeWidthStr;

use crate::tui::terminal_graph::{fit_label, GraphStyles, Oversize};

use super::MermaidArt;

const LABEL_CAP: usize = 20;
const LABEL_FLOOR: usize = 8;
const BAR_FLOOR: usize = 10;
const BAR_CAP: usize = 60;
const DEFAULT_DURATION: f32 = 3.0;

#[derive(Clone, Debug)]
pub(super) struct GanttModel {
    pub(super) title: Option<String>,
    pub(super) rows: Vec<GanttRow>,
    pub(super) axis: GanttAxis,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GanttAxis {
    Calendar { start: f32, end: f32 },
    Relative { start: f32, end: f32 },
}

impl GanttAxis {
    fn start(self) -> f32 {
        match self {
            Self::Calendar { start, .. } | Self::Relative { start, .. } => start,
        }
    }

    fn end(self) -> f32 {
        match self {
            Self::Calendar { end, .. } | Self::Relative { end, .. } => end,
        }
    }

    fn span(self) -> f32 {
        (self.end() - self.start()).max(1.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum GanttRow {
    Section(String),
    Task {
        label: String,
        start: f32,
        duration: f32,
        status: Option<GanttStatus>,
    },
}

pub(super) fn from_ir(ir: &mermaid_rs_renderer::Graph) -> Option<GanttModel> {
    if ir.gantt_tasks.is_empty() {
        return None;
    }
    Some(schedule(&ir.gantt_tasks, ir.gantt_title.clone()))
}

pub(super) fn complexity(ir: &mermaid_rs_renderer::Graph) -> (usize, usize, usize, usize) {
    (ir.gantt_tasks.len(), 0, ir.gantt_sections.len(), 0)
}

/// After-chain rules follow mermaid-rs-renderer 0.3.1: a task starts at its
/// parsed date, else at the end of the `after` predecessor, else at
/// origin + cursor. The parser lowercases `after` tokens and keeps declared
/// ids as written, so lookup is case-insensitive.
pub(super) fn schedule(tasks: &[GanttTask], title: Option<String>) -> GanttModel {
    let mut parsed_starts: HashMap<String, f32> = HashMap::new();
    let mut origin: Option<f32> = None;
    for task in tasks {
        if let Some(start) = task.start.as_deref().and_then(parse_gantt_date) {
            let start = start.num_days_from_ce() as f32;
            parsed_starts.insert(task_key(&task.id), start);
            origin = Some(origin.map_or(start, |value| value.min(start)));
        }
    }
    let calendar = origin.is_some();

    let mut timing: HashMap<String, (f32, f32)> = HashMap::new();
    let mut cursor = 0.0_f32;
    let mut time_start = f32::MAX;
    let mut time_end = f32::MIN;
    let mut rows = Vec::new();
    let mut current_section: Option<String> = None;
    for task in tasks {
        let duration = task
            .duration
            .as_deref()
            .and_then(parse_gantt_duration)
            .unwrap_or(DEFAULT_DURATION)
            .max(0.1);
        let start = parsed_starts
            .get(&task_key(&task.id))
            .copied()
            .or_else(|| {
                task.after
                    .as_deref()
                    .and_then(|after_id| timing.get(&task_key(after_id)).map(|(_, end)| *end))
            })
            .unwrap_or(origin.unwrap_or(0.0) + cursor);
        let end = start + duration;
        timing.insert(task_key(&task.id), (start, end));
        cursor = cursor.max(end + 0.5);
        time_start = time_start.min(start);
        time_end = time_end.max(end);
        if task.section != current_section {
            if let Some(name) = task.section.clone() {
                rows.push(GanttRow::Section(name));
            }
            current_section = task.section.clone();
        }
        rows.push(GanttRow::Task {
            label: task.label.clone(),
            start,
            duration,
            status: task.status,
        });
    }
    if !time_start.is_finite() || !time_end.is_finite() {
        time_start = 0.0;
        time_end = 1.0;
    }
    if (time_end - time_start).abs() < 0.01 {
        time_end = time_start + 1.0;
    }

    let axis = if calendar {
        GanttAxis::Calendar {
            start: time_start,
            end: time_end,
        }
    } else {
        GanttAxis::Relative {
            start: time_start,
            end: time_end,
        }
    };
    GanttModel { title, rows, axis }
}

pub(super) fn layout_gantt(
    model: &GanttModel,
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let longest_label = model
        .rows
        .iter()
        .filter_map(|row| match row {
            GanttRow::Task { label, .. } => Some(label.width()),
            GanttRow::Section(_) => None,
        })
        .max()
        .unwrap_or(0);
    let label_width = longest_label.clamp(LABEL_FLOOR, LABEL_CAP);
    let bar_width = match max_width {
        Some(max_width) => {
            let available = max_width.saturating_sub(label_width.saturating_add(1));
            if available < BAR_FLOOR {
                return Err(Oversize::Width);
            }
            available.min(BAR_CAP)
        }
        None => BAR_CAP,
    };
    let total_width = label_width
        .saturating_add(1)
        .saturating_add(bar_width)
        .max(1);
    if max_width.is_some_and(|max_width| total_width > max_width) {
        return Err(Oversize::Width);
    }

    let axis_span = model.axis.span();
    let mut lines = Vec::new();
    if let Some(title) = &model.title {
        lines.push(fit_label(title, total_width));
    }
    for row in &model.rows {
        match row {
            GanttRow::Section(name) => lines.push(section_line(name, total_width)),
            GanttRow::Task {
                label,
                start,
                duration,
                status,
            } => {
                let label = fit_label(label, label_width.max(1));
                let bar = bar_line(
                    *start,
                    *duration,
                    model.axis.start(),
                    axis_span,
                    bar_width,
                    *status,
                );
                lines.push(format!("{} {}", pad_end(&label, label_width), bar));
            }
        }
    }
    lines.push(axis_footer(model, label_width, bar_width, total_width));
    Ok(super::art_from_plain(lines, styles))
}

fn section_line(name: &str, width: usize) -> String {
    let label = fit_label(name, width.saturating_sub(4).max(1));
    let mut line = format!("── {label} ");
    while line.width() < width {
        line.push('─');
    }
    truncate_width(&line, width)
}

fn bar_line(
    start: f32,
    duration: f32,
    axis_start: f32,
    axis_span: f32,
    bar_width: usize,
    status: Option<GanttStatus>,
) -> String {
    if bar_width == 0 {
        return String::new();
    }
    let mut cells = vec![' '; bar_width];
    let start_col = map_col(start, axis_start, axis_span, bar_width);
    let end_col = map_col(start + duration, axis_start, axis_span, bar_width)
        .max(start_col.saturating_add(1))
        .min(bar_width);
    if status == Some(GanttStatus::Milestone) {
        cells[start_col.min(bar_width.saturating_sub(1))] = '◆';
        return cells.into_iter().collect();
    }
    let fill = match status {
        Some(GanttStatus::Done) => '░',
        Some(GanttStatus::Active) => '▓',
        Some(GanttStatus::Crit | GanttStatus::Milestone) | None => '█',
    };
    let mut fill_start = start_col;
    if status == Some(GanttStatus::Crit) {
        cells[start_col.min(bar_width.saturating_sub(1))] = '!';
        fill_start = start_col.saturating_add(1);
    }
    for cell in cells.iter_mut().take(end_col).skip(fill_start) {
        *cell = fill;
    }
    cells.into_iter().collect()
}

fn axis_footer(
    model: &GanttModel,
    label_width: usize,
    bar_width: usize,
    total_width: usize,
) -> String {
    let start = axis_label(model.axis.start(), model.axis);
    let end = axis_label(model.axis.end(), model.axis);
    let mut bar = vec![' '; bar_width];
    put_label(&mut bar, 0, &start);
    let end_at = bar_width.saturating_sub(end.width());
    if end_at > start.width() {
        put_label(&mut bar, end_at, &end);
    }
    let line = format!(
        "{} {}",
        " ".repeat(label_width),
        bar.into_iter().collect::<String>()
    );
    truncate_width(&line, total_width)
}

fn axis_label(value: f32, axis: GanttAxis) -> String {
    match axis {
        GanttAxis::Calendar { .. } => NaiveDate::from_num_days_from_ce_opt(value.round() as i32)
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| format!("{}", value.round() as i32)),
        GanttAxis::Relative { start, .. } => format!("{}d", (value - start).round() as i32),
    }
}

fn put_label(cells: &mut [char], start: usize, label: &str) {
    for (offset, character) in label.chars().enumerate() {
        if let Some(cell) = cells.get_mut(start + offset) {
            *cell = character;
        }
    }
}

fn map_col(value: f32, axis_start: f32, axis_span: f32, bar_width: usize) -> usize {
    let t = ((value - axis_start) / axis_span).clamp(0.0, 1.0);
    let col = (t * bar_width as f32).floor() as usize;
    col.min(bar_width.saturating_sub(1))
}

fn pad_end(label: &str, width: usize) -> String {
    let mut out = label.to_string();
    while out.width() < width {
        out.push(' ');
    }
    out
}

fn truncate_width(line: &str, width: usize) -> String {
    if line.width() <= width {
        return line.to_string();
    }
    fit_label(line, width)
}

fn task_key(id: &str) -> String {
    id.to_ascii_lowercase()
}

/// Mermaid duration suffixes, converted to days. `m` is minutes; `M` is months.
pub(super) fn parse_gantt_duration(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let split = value
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit() && *character != '.')
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    let number: f32 = value[..split].parse().ok()?;
    let suffix = value[split..].trim();
    let days = match suffix {
        "" | "d" => 1.0,
        "ms" => 1.0 / 86_400_000.0,
        "s" => 1.0 / 86_400.0,
        "m" => 1.0 / 1_440.0,
        "h" => 1.0 / 24.0,
        "w" => 7.0,
        "M" => 30.0,
        "y" => 365.0,
        _ => return None,
    };
    Some(number * days)
}

pub(super) fn parse_gantt_date(value: &str) -> Option<NaiveDate> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let parts: Vec<&str> = value.split(['-', '/', '.']).collect();
    if parts.len() != 3 {
        return None;
    }
    let year: i32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

#[cfg(test)]
#[path = "gantt_tests.rs"]
mod tests;
