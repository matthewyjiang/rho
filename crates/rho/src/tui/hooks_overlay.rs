//! `/hooks` overlay: the resolved spawn contract in the single-pane popup.
//!
//! Trusting a workspace means trusting the programs listed here, so every
//! field that decides what runs is on screen: files, the untrusted-file
//! notice, argv, working directory, timeout, and environment. Field values
//! share a column, and each argv element stays one token, so a wrapped path
//! cannot be read as another argument.

use std::time::Instant;

use ratatui::text::{Line, Span};

use super::{
    overlay_panel::{PanelBody, PanelState},
    panel_text::{heading_with_status, indented_wrapped_lines, truncate_to},
    render::{display_width, wrap_line_at_whitespace},
    theme::Theme,
    App, ComposerMode, PanelOverlay,
};
use crate::hooks::{HookContractView, HookReport};

const TITLE: &str = "Hooks";
const FOOTER: &str = "Enter/Esc close";
const NO_FILES: &str = "no hooks files found";
const NO_HOOKS: &str = "no hooks are configured";
/// File paths and hook ids sit under the section heading.
const ROW_INDENT: usize = 2;
/// Contract fields sit under the hook id.
const FIELD_INDENT: usize = 4;
const FIELD_GAP: usize = 2;
const FIELD_LABELS: &[&str] = &["tools", "argv", "directory", "timeout", "environment"];

#[derive(Clone, Debug, PartialEq)]
pub(super) struct HooksOverlay {
    report: HookReport,
    panel: PanelState,
}

impl PanelBody for HooksOverlay {
    fn state(&self) -> &PanelState {
        &self.panel
    }

    fn state_mut(&mut self) -> &mut PanelState {
        &mut self.panel
    }

    fn title(&self) -> &str {
        TITLE
    }

    fn footer(&self) -> &str {
        FOOTER
    }

    fn body_lines(&self, width: usize, _now: Instant) -> Vec<Line<'static>> {
        hooks_body_lines(&self.report, width)
    }
}

impl App {
    pub(super) fn show_hooks_report(&mut self, report: HookReport) {
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Hooks(HooksOverlay {
                report,
                panel: PanelState::default(),
            })));
        self.set_status_quiet("hooks");
    }
}

fn hooks_body_lines(report: &HookReport, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![heading_with_status(
        "Files",
        &count_status(report.files.len()),
        width,
    )];
    if report.files.is_empty() {
        lines.extend(indented_wrapped_lines(
            NO_FILES,
            ROW_INDENT,
            width,
            Theme::dim(),
        ));
    } else {
        for file in &report.files {
            lines.extend(indented_wrapped_lines(
                file,
                ROW_INDENT,
                width,
                Theme::text(),
            ));
        }
    }
    if report.skipped_untrusted.is_some() || report.skipped_untrusted_error.is_some() {
        lines.push(Line::default());
    }
    if let Some(skipped) = &report.skipped_untrusted {
        lines.extend(indented_wrapped_lines(
            &format!("ignoring {skipped}"),
            ROW_INDENT,
            width,
            Theme::warning(),
        ));
        lines.extend(indented_wrapped_lines(
            &format!(
                "workspace is not trusted; set {}=1 to load it",
                crate::hooks::TRUST_PROJECT_HOOKS_ENV
            ),
            ROW_INDENT,
            width,
            Theme::warning(),
        ));
    }
    if let Some(error) = &report.skipped_untrusted_error {
        lines.extend(indented_wrapped_lines(
            &format!("could not inspect untrusted hooks: {error}"),
            ROW_INDENT,
            width,
            Theme::error(),
        ));
    }

    lines.push(Line::default());
    lines.push(heading_with_status(
        "Hooks",
        &count_status(report.hooks.len()),
        width,
    ));
    if report.hooks.is_empty() {
        lines.extend(indented_wrapped_lines(
            NO_HOOKS,
            ROW_INDENT,
            width,
            Theme::dim(),
        ));
        return lines;
    }
    for (index, hook) in report.hooks.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.extend(hook_lines(hook, width));
    }
    lines
}

fn hook_lines(hook: &HookContractView, width: usize) -> Vec<Line<'static>> {
    let indent = " ".repeat(ROW_INDENT.min(width));
    let mut lines = vec![heading_with_status(
        &format!("{indent}{}", hook.id),
        &hook.event,
        width,
    )];
    if !hook.active {
        lines.extend(indented_wrapped_lines(
            "inactive",
            FIELD_INDENT,
            width,
            Theme::warning(),
        ));
    }
    let label_width = FIELD_LABELS
        .iter()
        .copied()
        .map(display_width)
        .max()
        .unwrap_or(0);
    let layout = field_layout(width, label_width);
    let values = [
        hard_wrap(&hook.tools, layout.value_width),
        argv_rows(&hook.command, layout.value_width),
        hard_wrap(&hook.working_directory, layout.value_width),
        hard_wrap(&hook.timeout, layout.value_width),
        pack_names(&hook.environment, layout.value_width),
    ];
    debug_assert_eq!(FIELD_LABELS.len(), values.len());
    for (label, rows) in FIELD_LABELS.iter().copied().zip(values) {
        lines.extend(labeled_rows(label, rows, &layout));
    }
    lines
}

/// Columns for one contract field. The label yields before the value does.
struct FieldLayout {
    indent: usize,
    label_width: usize,
    value_at: usize,
    value_width: usize,
}

fn field_layout(width: usize, label_width: usize) -> FieldLayout {
    let width = width.max(1);
    let gap = FIELD_GAP.min(width.saturating_sub(2));
    let indent = FIELD_INDENT.min(width.saturating_sub(gap + 1));
    let label_width = label_width.min(width.saturating_sub(indent + gap + 1));
    let value_at = indent + label_width + gap;
    FieldLayout {
        indent,
        label_width,
        value_at,
        value_width: width.saturating_sub(value_at).max(1),
    }
}

impl FieldLayout {
    fn prefix(&self, label: &str) -> String {
        let indent = " ".repeat(self.indent.min(self.value_at));
        let shown = if self.label_width == 0 {
            String::new()
        } else {
            truncate_to(label, self.label_width)
        };
        let mut prefix = format!("{indent}{shown}");
        let used = display_width(&prefix);
        if used < self.value_at {
            prefix.push_str(&" ".repeat(self.value_at - used));
        }
        prefix
    }
}

fn labeled_rows(label: &str, mut rows: Vec<String>, layout: &FieldLayout) -> Vec<Line<'static>> {
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let value = truncate_to(&row, layout.value_width);
            if index == 0 {
                Line::from(vec![
                    Span::styled(layout.prefix(label), Theme::dim()),
                    Span::styled(value, Theme::text()),
                ])
            } else {
                let indent = " ".repeat(layout.value_at);
                Line::from(Span::styled(format!("{indent}{value}"), Theme::text()))
            }
        })
        .collect()
}

/// One argv element per row. A space inside an argument is quoted so it cannot
/// be read as a second argument, including when the token itself wraps.
fn argv_rows(command: &[String], value_width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for arg in command {
        rows.extend(hard_wrap(&display_argv(arg), value_width));
    }
    rows
}

fn display_argv(arg: &str) -> String {
    let literal = |ch: char| ch.is_ascii_graphic() && ch != '\'';
    if !arg.is_empty() && arg.chars().all(literal) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Pack environment names onto rows. A name splits only when it is wider than
/// the value column.
fn pack_names(names: &[String], value_width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    for name in names {
        append_token(&mut rows, &mut current, name, value_width);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

fn append_token(rows: &mut Vec<String>, current: &mut String, token: &str, value_width: usize) {
    let token_width = display_width(token);
    if current.is_empty() {
        if token_width <= value_width {
            current.push_str(token);
        } else {
            rows.extend(hard_wrap(token, value_width));
        }
        return;
    }
    if display_width(current) + 1 + token_width <= value_width {
        current.push(' ');
        current.push_str(token);
        return;
    }
    rows.push(std::mem::take(current));
    if token_width <= value_width {
        current.push_str(token);
    } else {
        rows.extend(hard_wrap(token, value_width));
    }
}

fn hard_wrap(value: &str, value_width: usize) -> Vec<String> {
    wrap_line_at_whitespace(value, value_width.max(1))
        .into_iter()
        .map(|part| part.trim_start().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn count_status(count: usize) -> String {
    if count == 0 {
        "none".into()
    } else {
        count.to_string()
    }
}

#[cfg(test)]
#[path = "hooks_overlay_tests.rs"]
mod tests;
