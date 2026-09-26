//! Pure layout for the `/spend` overlay body.
//!
//! Everything here turns an already-aggregated [`SpendReport`] into styled
//! lines at a given width. Loading, keys, and range selection live in
//! `spend_overlay`.

use chrono::NaiveDateTime;
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use super::{
    panel_text::{indented_wrapped_lines, truncate_to},
    render::display_width,
    theme::Theme,
    usage_cost::format_token_count as count,
};
use crate::usage::report::{
    SpendGroup, SpendRange, SpendReport, SpendTotals, Timeline, TimelineUnit, Valuation,
};

const INDENT: &str = "  ";
const COST_WIDTH: usize = 12;
const TOKENS_WIDTH: usize = 9;
/// Rows shown per table before the rest fold into one `N more` row.
const MAX_TABLE_ROWS: usize = 5;
/// Body width cap: past this, names and numbers drift too far apart to scan.
const MAX_BODY_WIDTH: usize = 72;
/// Chart columns per bucket when a short range would leave the chart tiny.
const MAX_BUCKET_COLUMNS: usize = 6;
const SPARK_LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

fn range_label(range: SpendRange) -> &'static str {
    match range {
        SpendRange::AllTime => "All time",
        SpendRange::Last30Days => "30 days",
        SpendRange::Last7Days => "7 days",
        SpendRange::Today => "Today",
    }
}

/// Range selector: the active range as a filled pill, the rest dim.
pub(super) fn range_tabs_line(selected: SpendRange) -> Line<'static> {
    let mut spans = vec![Span::raw(INDENT)];
    for (index, range) in SpendRange::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let label = format!(" {} ", range_label(range));
        let style = if range == selected {
            Theme::brand().add_modifier(Modifier::REVERSED)
        } else {
            Theme::dim()
        };
        spans.push(Span::styled(label, style));
    }
    Line::from(spans)
}

/// The report body below the range tabs: a headline total with one detail
/// line, a full-width chart, then model, provider, and purpose tables.
pub(super) fn spend_body_lines(
    range: SpendRange,
    report: &SpendReport,
    width: usize,
) -> Vec<Line<'static>> {
    let width = width.min(MAX_BODY_WIDTH);
    if report.totals.requests == 0 {
        let message = match range {
            SpendRange::AllTime => {
                "No model requests recorded yet. Rho writes one row per request to the usage ledger."
            }
            SpendRange::Today | SpendRange::Last7Days | SpendRange::Last30Days => {
                "No model requests in this range. Press Tab for a longer range."
            }
        };
        return indented_wrapped_lines(message, INDENT.len(), width, Theme::dim());
    }

    let mut lines = summary_lines(report, width);
    lines.push(Line::default());
    lines.extend(timeline_lines(&report.timeline, width));
    let name_width = width
        .saturating_sub(INDENT.len() + COST_WIDTH + TOKENS_WIDTH)
        .max(1);
    for (title, groups) in [
        ("Models", &report.models),
        ("Providers", &report.providers),
        ("Purpose", &report.purposes),
    ] {
        lines.push(Line::default());
        lines.push(section_heading(title, width));
        let (shown, rest) = fold_tail(groups, MAX_TABLE_ROWS);
        for group in shown {
            lines.push(table_row(name_width, &group.name, &group.totals));
        }
        if let Some((hidden, totals)) = rest {
            lines.push(
                table_row(name_width, &format!("{hidden} more"), &totals).patch_style(Theme::dim()),
            );
        }
    }
    lines
}

/// Headline total and one dim line: the actual/computed split, then request
/// and unpriced counts. The whole history story fits one line at 60 columns.
fn summary_lines(report: &SpendReport, width: usize) -> Vec<Line<'static>> {
    let totals = &report.totals;
    let value = money(totals.equivalent_usd_micros());
    let label = "Equivalent API cost";
    let pad = width
        .saturating_sub(display_width(label))
        .saturating_sub(display_width(&value));
    let headline = Line::from(vec![
        Span::styled(label, Theme::text_strong()),
        Span::raw(" ".repeat(pad)),
        Span::styled(value, Theme::brand()),
    ]);

    let mut detail = vec![
        format!("{} actual", money(totals.actual_usd_micros)),
        format!("{} computed", money(totals.computed_usd_micros)),
        plural(totals.requests, "request"),
    ];
    if totals.unpriced_requests > 0 {
        detail.push(format!("{} unpriced", count(totals.unpriced_requests)));
    }
    // Drop trailing parts that do not fit rather than cutting one mid-word.
    let mut text = String::from(INDENT);
    for (index, part) in detail.iter().enumerate() {
        let separator = if index == 0 { "" } else { " · " };
        if display_width(&text) + display_width(separator) + display_width(part) > width {
            break;
        }
        text.push_str(separator);
        text.push_str(part);
    }
    vec![headline, Line::from(Span::styled(text, Theme::dim()))]
}

/// Full-width spend chart with its range and peak on one dim axis line.
fn timeline_lines(timeline: &Timeline, width: usize) -> Vec<Line<'static>> {
    let values = &timeline.equivalent_usd_micros;
    let columns = chart_columns(values, width.saturating_sub(INDENT.len()));
    // Columns hold single-bucket values, so the chart peak is the labeled one.
    let peak = values.iter().copied().max().unwrap_or(0);

    let mut chart = vec![Span::raw(INDENT)];
    for value in &columns {
        let (glyph, style) = if *value == 0 {
            ("▁", Theme::dim())
        } else {
            let level = ((*value as u128 * 8).div_ceil(peak as u128) as usize).clamp(1, 8);
            (SPARK_LEVELS[level - 1], Theme::accent())
        };
        chart.push(Span::styled(glyph, style));
    }

    let per = match timeline.unit {
        TimelineUnit::Hour => "hour",
        TimelineUnit::Day => "day",
    };
    let start = bucket_label(timeline.unit, timeline.start);
    let end = if peak > 0 {
        format!("peak {} / {per}", money(peak))
    } else {
        String::new()
    };
    let gap = columns
        .len()
        .saturating_sub(display_width(&start))
        .saturating_sub(display_width(&end))
        .max(2);
    let axis = format!("{INDENT}{start}{}{end}", " ".repeat(gap));
    vec![
        Line::from(chart),
        Line::from(Span::styled(truncate_to(&axis, width), Theme::dim())),
    ]
}

/// One value per chart column, mapping columns to buckets proportionally so
/// the first and last buckets are always drawn. A column spanning several
/// buckets shows their max, so merged columns match single-bucket heights.
/// Short ranges get at most [`MAX_BUCKET_COLUMNS`] columns per bucket.
fn chart_columns(values: &[u64], width: usize) -> Vec<u64> {
    let count = width.min(values.len() * MAX_BUCKET_COLUMNS);
    (0..count)
        .map(|column| {
            let from = column * values.len() / count;
            let to = ((column + 1) * values.len() / count).max(from + 1);
            values[from..to].iter().copied().max().unwrap_or(0)
        })
        .collect()
}

fn bucket_label(unit: TimelineUnit, start: NaiveDateTime) -> String {
    match unit {
        TimelineUnit::Hour => start.format("%H:%M").to_string(),
        TimelineUnit::Day => start.format("%b %-d").to_string(),
    }
}

/// Bold section title; the cost and token columns explain themselves.
fn section_heading(title: &str, width: usize) -> Line<'static> {
    Line::from(Span::styled(
        truncate_to(title, width),
        Theme::text_strong(),
    ))
}

/// Split `groups` into the first `max` rows and, when more remain, a count
/// and combined totals for the rest. Folding never drops spend.
fn fold_tail(groups: &[SpendGroup], max: usize) -> (&[SpendGroup], Option<(usize, SpendTotals)>) {
    // Folding a single row into `1 more` saves nothing.
    if groups.len() <= max + 1 {
        return (groups, None);
    }
    let (shown, hidden) = groups.split_at(max);
    let mut totals = SpendTotals::default();
    for group in hidden {
        totals += group.totals;
    }
    (shown, Some((hidden.len(), totals)))
}

/// Name, cost, and token columns for the model, provider, and purpose tables.
/// The cost cell is `local` for local-only rows and a dim `—` when nothing
/// could be priced; the summary line carries the unpriced count.
fn table_row(name_width: usize, name: &str, totals: &SpendTotals) -> Line<'static> {
    let name = truncate_to(name, name_width.saturating_sub(1));
    let pad = name_width.saturating_sub(display_width(&name));
    let (cost, cost_style) = match totals.valuation() {
        Valuation::Local => ("local".to_owned(), Theme::dim()),
        Valuation::Unpriced => ("—".to_owned(), Theme::dim()),
        Valuation::Usd(micros) => (money(micros), Theme::text()),
    };
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(name, Theme::text()),
        Span::raw(" ".repeat(pad)),
        Span::styled(format!("{cost:>COST_WIDTH$}"), cost_style),
        Span::styled(
            format!("{:>TOKENS_WIDTH$}", count(totals.tokens)),
            Theme::dim(),
        ),
    ])
}

/// US dollars with cents and thousands separators; sub-cent spend stays visible.
fn money(micros: u64) -> String {
    if micros > 0 && micros < 10_000 {
        return "<$0.01".to_owned();
    }
    let cents = micros.saturating_add(5_000) / 10_000;
    format!("${}.{:02}", grouped(cents / 100), cents % 100)
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn plural(value: u64, noun: &str) -> String {
    let suffix = if value == 1 { "" } else { "s" };
    format!("{} {noun}{suffix}", count(value))
}

#[cfg(test)]
#[path = "spend_view_tests.rs"]
mod tests;
