use std::collections::HashMap;

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
    pub(super) axis_start: f32,
    pub(super) axis_end: f32,
    pub(super) has_dates: bool,
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

pub(super) fn schedule(tasks: &[GanttTask], title: Option<String>) -> GanttModel {
    let mut parsed_starts: HashMap<String, f32> = HashMap::new();
    let mut origin: Option<f32> = None;
    for task in tasks {
        if let Some(start) = task.start.as_deref().and_then(parse_gantt_date) {
            let start = start as f32;
            parsed_starts.insert(task.id.clone(), start);
            origin = Some(origin.map_or(start, |value| value.min(start)));
        }
    }
    let has_dates = origin.is_some();

    let mut timing: HashMap<String, (f32, f32)> = HashMap::new();
    let mut cursor = 0.0_f32;
    let mut time_start = f32::MAX;
    let mut time_end = f32::MIN;
    let mut computed = Vec::with_capacity(tasks.len());
    for task in tasks {
        let duration = task
            .duration
            .as_deref()
            .and_then(parse_gantt_duration)
            .unwrap_or(DEFAULT_DURATION)
            .max(0.1);
        let mut start = parsed_starts.get(&task.id).copied();
        if start.is_none() {
            if let Some(end) = task
                .after
                .as_deref()
                .and_then(|after_id| timing.get(after_id).map(|(_, end)| *end))
            {
                start = Some(end);
            }
        }
        let fallback_base = origin.unwrap_or(0.0);
        let start = start.unwrap_or(fallback_base + cursor);
        let end = start + duration;
        timing.insert(task.id.clone(), (start, end));
        cursor = cursor.max(end + 0.5);
        time_start = time_start.min(start);
        time_end = time_end.max(end);
        computed.push((
            task.label.clone(),
            start,
            duration,
            task.status,
            task.section.clone(),
        ));
    }
    if !time_start.is_finite() || !time_end.is_finite() {
        time_start = 0.0;
        time_end = 1.0;
    }
    if (time_end - time_start).abs() < 0.01 {
        time_end = time_start + 1.0;
    }

    let mut rows = Vec::new();
    let mut current_section: Option<String> = None;
    for (label, start, duration, status, section) in computed {
        if section != current_section {
            if let Some(name) = section.clone() {
                rows.push(GanttRow::Section(name));
            }
            current_section = section;
        }
        rows.push(GanttRow::Task {
            label,
            start,
            duration,
            status,
        });
    }

    GanttModel {
        title,
        rows,
        axis_start: time_start,
        axis_end: time_end,
        has_dates,
    }
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
    let label_width = longest_label.min(LABEL_CAP);
    let bar_width = match max_width {
        Some(max_width) => {
            let available = max_width.saturating_sub(label_width.saturating_add(1));
            if label_width < LABEL_FLOOR || available < BAR_FLOOR {
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

    let axis_span = (model.axis_end - model.axis_start).max(1.0);
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
                    model.axis_start,
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
    let start = axis_label(model.axis_start, model.has_dates, model.axis_start);
    let end = axis_label(model.axis_end, model.has_dates, model.axis_start);
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

fn axis_label(value: f32, has_dates: bool, origin: f32) -> String {
    if has_dates {
        format_gantt_date(value.round() as i32)
    } else {
        format!("{}d", (value - origin).round() as i32)
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

pub(super) fn parse_gantt_duration(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut digits = String::new();
    let mut unit = None;
    for character in value.chars() {
        if character.is_ascii_digit() || character == '.' {
            digits.push(character);
        } else if !character.is_whitespace() {
            unit = Some(character.to_ascii_lowercase());
        }
    }
    let number: f32 = digits.parse().ok()?;
    let mult = match unit {
        Some('d') => 1.0,
        Some('w') => 7.0,
        Some('h') => 1.0 / 24.0,
        Some('m') => 30.0,
        Some('y') => 365.0,
        _ => 1.0,
    };
    Some(number * mult)
}

pub(super) fn parse_gantt_date(value: &str) -> Option<i32> {
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
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i32 {
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i32;
    let d = day as i32;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(days: i32) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i32::from(m <= 2);
    (year, m as u32, d as u32)
}

fn format_gantt_date(days: i32) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
#[path = "gantt_tests.rs"]
mod tests;
