//! `/context` overlay: what fills the context window, grouped by source.
//!
//! The runtime builds the report on demand (it scans history), so the command
//! runs only while idle and the panel is a snapshot. `c` copies the report.

use std::time::Instant;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use rho_providers::model::ModelMetadata;

use crate::app::context_report::{ContextBasis, ContextReport};

use super::{
    command_block::CommandBlock,
    overlay_panel::{PanelBody, PanelState},
    render::{display_width, truncate_one_line},
    statusline::path::shorten_path_display,
    theme::Theme,
    usage_cost::{context_fill_percent, format_token_count},
    App, ComposerMode, InteractiveRuntime, PanelOverlay,
};

const TITLE: &str = "Context";
const FOOTER: &str = "c copy  Enter/Esc close";
/// Right-aligned columns: token count ("999.9K") and share of the total ("100.0%").
const TOKENS_WIDTH: usize = 8;
const PERCENT_WIDTH: usize = 8;
/// Narrower label room than this stacks the numbers under each label.
const MIN_LABEL_WIDTH: usize = 12;
/// Wide enough that a pasted row stays on one line.
const COPY_WIDTH: usize = 160;

#[derive(Clone, Debug)]
pub(super) struct ContextOverlay {
    report: ContextReport,
    /// Effective limit: the runtime's window, else the model catalog's.
    window: Option<u64>,
    panel: PanelState,
}

impl PanelBody for ContextOverlay {
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
        context_lines(&self.report, self.window, width)
    }

    fn copy_text(&self) -> Option<String> {
        Some(
            context_lines(&self.report, self.window, COPY_WIDTH)
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

impl App {
    pub(super) fn execute_context_command(
        &mut self,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let report = agent.context_report();
        let window = report
            .window
            .filter(|window| *window > 0)
            .or_else(|| {
                self.model_metadata
                    .as_ref()
                    .and_then(ModelMetadata::display_context_window)
            })
            .filter(|window| *window > 0);
        self.input_ui
            .set_composer(ComposerMode::Panel(PanelOverlay::Context(Box::new(
                ContextOverlay {
                    report,
                    window,
                    panel: PanelState::default(),
                },
            ))));
        self.set_status_quiet("context");
        Ok(())
    }
}

fn context_lines(report: &ContextReport, window: Option<u64>, width: usize) -> Vec<Line<'static>> {
    let mut header = CommandBlock::on_surface(width);
    let total = format_token_count(report.tokens);
    let title = match window {
        Some(window) => format!(
            "≈{total} / {} tokens ({:.1}%)",
            format_token_count(window),
            context_fill_percent(report.tokens, window)
        ),
        None => format!("≈{total} tokens"),
    };
    header.push_header(
        &title,
        match report.basis {
            ContextBasis::ProviderCalibrated => "calibrated to the last provider-reported prompt",
            ContextBasis::LocalEstimate => "local estimate",
        },
    );
    header
        .push_note("Rows are local estimates (about 4 characters per token) scaled to the total.");
    let mut lines = header.finish();
    if report.groups.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            truncate_one_line("  Nothing in context yet.", width),
            Theme::dim(),
        )));
        return lines;
    }

    let table = Table {
        width,
        total: report.tokens,
    };
    for group in &report.groups {
        lines.push(Line::default());
        lines.extend(table.row(
            group.label,
            None,
            group.tokens,
            Theme::text().add_modifier(Modifier::BOLD),
        ));
        for row in &group.rows {
            lines.extend(table.row(
                &format!("  {}", row.label),
                row.detail.as_deref(),
                row.tokens,
                Theme::text(),
            ));
        }
    }
    if let Some(window) = window {
        // Free space is a share of the window, not of the used total.
        let table = Table {
            width,
            total: window,
        };
        lines.push(Line::default());
        lines.extend(table.row(
            "Free space",
            None,
            window.saturating_sub(report.tokens),
            Theme::dim(),
        ));
    }
    lines
}

/// Label column on the left, token and percent columns aligned on the right.
/// When the label column would be too narrow, numbers move to their own line.
struct Table {
    width: usize,
    total: u64,
}

impl Table {
    fn row(
        &self,
        label: &str,
        detail: Option<&str>,
        tokens: u64,
        style: Style,
    ) -> Vec<Line<'static>> {
        let percent = if self.total == 0 {
            0.0
        } else {
            context_fill_percent(tokens, self.total)
        };
        let tokens = format_token_count(tokens);
        let percent = format!("{percent:.1}%");
        let numbers = format!("{tokens:>TOKENS_WIDTH$}{percent:>PERCENT_WIDTH$}");
        let label_width = self.width.saturating_sub(display_width(&numbers) + 1);
        if label_width < MIN_LABEL_WIDTH {
            let indent = label.len() - label.trim_start().len() + 2;
            let numbers = format!("{}{tokens}  {percent}", " ".repeat(indent));
            return vec![
                Line::from(Span::styled(truncate_one_line(label, self.width), style)),
                Line::from(Span::styled(truncate_one_line(&numbers, self.width), style)),
            ];
        }
        let label = truncate_one_line(label, label_width);
        let mut spans = vec![Span::styled(label.clone(), style)];
        let mut used = display_width(&label);
        if let Some(detail) = detail {
            let room = label_width.saturating_sub(used + 2);
            if room >= 4 {
                let detail = shorten_path_display(detail, room);
                used += 2 + display_width(&detail);
                spans.push(Span::styled(format!("  {detail}"), Theme::dim()));
            }
        }
        if used + display_width(&numbers) < self.width {
            let padding = self.width - used - display_width(&numbers);
            spans.push(Span::styled(" ".repeat(padding), style));
            spans.push(Span::styled(numbers, style));
        }
        vec![Line::from(spans)]
    }
}
