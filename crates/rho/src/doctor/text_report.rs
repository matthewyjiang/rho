//! Plain-text rendering of a [`DoctorReport`] for `rho doctor`.
//!
//! Status words are ASCII so the output stays grep-friendly. Hints are always
//! printed; the interactive overlay is the surface that hides them for
//! healthy rows.

use super::report::DoctorReport;

pub(crate) fn render(report: &DoctorReport) -> String {
    let mut out = format!("Doctor: {}\n", report.headline());
    let word_width = report
        .checks()
        .map(|check| check.status.word().len())
        .max()
        .unwrap_or(0);
    let label_width = report
        .checks()
        .map(|check| check.label.chars().count())
        .max()
        .unwrap_or(0);
    let hint_indent = " ".repeat(4 + word_width);
    for section in &report.sections {
        out.push('\n');
        out.push_str(section.id.label());
        out.push('\n');
        for check in &section.checks {
            out.push_str(&format!(
                "  {:word_width$}  {:label_width$}  {}\n",
                check.status.word(),
                check.label,
                check.summary
            ));
            if let Some(hint) = &check.hint {
                out.push_str(&format!("{hint_indent}{hint}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "text_report_tests.rs"]
mod tests;
